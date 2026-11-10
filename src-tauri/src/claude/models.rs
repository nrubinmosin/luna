//! The models Claude Code offers an account, with the effort levels each
//! takes — what `/model` inside Claude Code shows, so a new model family or a
//! new effort level reaches the new-chat dialog without a Luna release.
//!
//! The list is the account's own: plan and organization decide what is on
//! it, so no other account's list ever stands in for it (unlike Codex's,
//! codex/models.rs). It comes from wherever it can be had soonest:
//! 1. `<CLAUDE_CONFIG_DIR>/cache/model-catalog/*-cc.json`, the catalog the
//!    CLI keeps for the account, while its `staleAt` has not passed;
//! 2. what the CLI said in answer to the SDK's `initialize` control request
//!    (`Query.supportedModels()`), asked on the account in the background
//!    and remembered by Luna in `claude-models.json` in its data folder; a
//!    stale catalog is returned at once meanwhile, and refreshed behind;
//! 3. Claude Code's four aliases, as Luna listed them before it asked.
//!
//! Both formats are the CLI's, and the catalog file is not documented: each
//! is read strictly, and anything unexpected skips that source (logged once
//! per account and source) rather than offer a half-read list.
//!
//! What Luna passes to `--model` (`slug`): a family's main model goes by its
//! alias (`opus`), so a chat follows the family to its next version as the
//! CLI does; any other model by its full id (`claude-opus-4-7`). A catalog
//! model gets the alias when it is in the `main` section and the alias its
//! `short_name` spells is one `provider_alias_targets` points at it.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::boundary::{AccountPath, OrEmpty};
use crate::provider::Provider;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeModel {
    /// What `--model` is given: an alias or a full id (module doc).
    pub slug: String,
    /// The full id the slug stands for, where the source says.
    pub model_id: Option<String>,
    pub display_name: String,
    pub default_effort: Option<String>,
    /// Empty for a model that takes no effort.
    pub efforts: Vec<String>,
    /// What the catalog says of a level, by level.
    pub effort_hints: BTreeMap<String, String>,
    /// Listed under "More…": an older version, not a family's main model.
    pub more: bool,
}

/// The aliases Luna listed before it asked the CLI, and lists still where it
/// cannot. Order as `/model` has them.
pub const ALIASES: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];

/// The levels the alias fallback offers: `--effort` of Claude Code 2.1.322.
const STOCK_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

fn aliases() -> Vec<ClaudeModel> {
    ALIASES
        .iter()
        .map(|a| ClaudeModel {
            slug: (*a).to_owned(),
            model_id: None,
            display_name: capitalized(a),
            default_effort: None,
            efforts: STOCK_EFFORTS.iter().map(|e| (*e).to_owned()).collect(),
            effort_hints: BTreeMap::new(),
            more: false,
        })
        .collect()
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Effort level ids as the CLI spells them: lower-case words, nothing else.
fn level(v: &serde_json::Value) -> Option<String> {
    let s = v.as_str()?;
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase())).then(|| s.to_owned())
}

fn levels(v: &serde_json::Value) -> Option<Vec<String>> {
    v.as_array()?.iter().map(level).collect()
}

/// An alias as `--model` takes it: one lower-case word.
fn is_alias(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase())
}

// --------------------------------------------------------------- catalog --

/// The catalog file as Claude Code 2.1.322 writes it: `version` 2, `fetchedAt`
/// and `staleAt` in ms, `catalog.config.models[]`, and
/// `catalog.config.provider_alias_targets`. None on any surprise.
fn parse_catalog(text: &str) -> Option<Catalog> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v["version"].as_u64() != Some(2) || v["catalog"]["surface"].as_str() != Some("cc") {
        return None;
    }
    let fetched_at = v["fetchedAt"].as_u64()?;
    let stale_at = v["staleAt"].as_u64()?;
    let config = &v["catalog"]["config"];
    // Each alias and the model it means where the account talks to Anthropic
    // itself; `per_provider` is for Bedrock, Vertex and the like, which Luna's
    // accounts do not run on.
    let targets: HashMap<&str, &str> = config["provider_alias_targets"]
        .as_object()?
        .iter()
        .map(|(alias, t)| Some((alias.as_str(), t["default"].as_str()?)))
        .collect::<Option<_>>()?;
    let models = config["models"]
        .as_array()?
        .iter()
        .map(|m| {
            let id = m["id"].as_str().filter(|s| !s.is_empty())?;
            let name = m["name"].as_str().filter(|s| !s.is_empty())?;
            let more = match m["section"].as_str()? {
                "main" => false,
                "overflow" => true,
                _ => return None,
            };
            let thinking = &m["thinking"];
            let (efforts, hints, default_effort) = match thinking["type"].as_str() {
                Some("none") => (Vec::new(), BTreeMap::new(), None),
                Some(_) => {
                    let options = thinking["effort_options"].as_array();
                    let efforts = match &m["runtime"]["effort_levels"] {
                        serde_json::Value::Null => options?.iter().map(|o| level(&o["id"])).collect::<Option<Vec<_>>>()?,
                        l => levels(l)?,
                    };
                    let hints: BTreeMap<String, String> = options
                        .into_iter()
                        .flatten()
                        .filter_map(|o| {
                            let hint = o["tooltip"]["content"].as_str().or_else(|| o["badge"]["message"].as_str())?;
                            Some((o["id"].as_str()?.to_owned(), hint.to_owned()))
                        })
                        .collect();
                    let default = match &m["runtime"]["default_effort"] {
                        serde_json::Value::Null => None,
                        d => Some(level(d)?),
                    };
                    (efforts, hints, default)
                }
                None => return None,
            };
            let alias = m["short_name"].as_str().map(str::to_lowercase).filter(|a| is_alias(a));
            let slug = match alias {
                Some(a) if !more && targets.get(a.as_str()) == Some(&id) => a,
                _ => id.to_owned(),
            };
            Some(ClaudeModel {
                slug,
                model_id: Some(id.to_owned()),
                display_name: name.to_owned(),
                default_effort,
                efforts,
                effort_hints: hints,
                more,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    (!models.is_empty()).then_some(Catalog { models, fetched_at, stale_at })
}

struct Catalog {
    models: Vec<ClaudeModel>,
    fetched_at: u64,
    stale_at: u64,
}

/// The account's catalog: the newest `*-cc.json` in its cache folder. None
/// when there is none; a file there that does not read is logged.
fn read_catalog(account_path: &str) -> Option<Catalog> {
    let dir = Path::new(account_path).join("cache").join("model-catalog");
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with("-cc.json"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    let (_, newest) = files.into_iter().next()?;
    let parsed = std::fs::read_to_string(&newest).ok().and_then(|t| parse_catalog(&t));
    if parsed.is_none() {
        surprise(account_path, "catalog", &format!("{} is not in the format Luna knows; skipping it", newest.display()));
    }
    parsed
}

// ------------------------------------------------------------ initialize --

/// The `initialize` control response as Claude Code 2.1.322 writes it on
/// stream-json stdout: `response.response.models[]` of the SDK's `ModelInfo`
/// (`value`, `displayName`, `resolvedModel`, `supportsEffort`,
/// `supportedEffortLevels`), with `account` saying whether it was the
/// account's list at all: signed out it is `{tokenSource: "none", …}`,
/// signed in `{email, subscriptionType, apiProvider}` with no `tokenSource`.
/// None on any surprise; `Some((models, signed_in))` otherwise.
fn parse_initialize(line: &str, request_id: &str) -> Option<(Vec<ClaudeModel>, bool)> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v["type"] != "control_response" || v["response"]["request_id"] != request_id || v["response"]["subtype"] != "success" {
        return None;
    }
    let r = &v["response"]["response"];
    let account = r["account"].as_object()?;
    let signed_in = account.get("tokenSource").and_then(|t| t.as_str()) != Some("none") && account.contains_key("subscriptionType");
    let models = r["models"]
        .as_array()?
        .iter()
        // The CLI's own "Default (recommended)" entry: the model it stands
        // for is listed under its own name too.
        .filter(|m| m["value"] != "default")
        .map(|m| {
            let value = m["value"].as_str().filter(|s| !s.is_empty())?;
            let name = m["displayName"].as_str().filter(|s| !s.is_empty())?;
            let efforts = match (&m["supportsEffort"], &m["supportedEffortLevels"]) {
                (serde_json::Value::Bool(false), _) | (_, serde_json::Value::Null) => Vec::new(),
                (_, l) => levels(l)?,
            };
            Some(ClaudeModel {
                slug: value.to_owned(),
                model_id: m["resolvedModel"].as_str().map(str::to_owned),
                display_name: name.to_owned(),
                default_effort: None,
                efforts,
                effort_hints: BTreeMap::new(),
                more: !is_alias(value),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    (!models.is_empty()).then_some((models, signed_in))
}

/// The line that asks: what the SDK sends first, before any prompt.
fn initialize_request(request_id: &str) -> String {
    serde_json::json!({ "type": "control_request", "request_id": request_id, "request": { "subtype": "initialize" } }).to_string()
}

/// How long the CLI gets to answer: it starts, reads the account, may renew
/// its token, and answers before any turn.
const ASK_TIMEOUT: Duration = Duration::from_secs(20);

/// Claude Code started headless on the account, asked `initialize` and
/// stopped once it has answered: no prompt is sent, so no turn runs and no
/// conversation is written. MCP servers are not started (`--strict-mcp-config`
/// with none given): the answer does not need them.
fn from_cli(account_path: &str) -> Option<(Vec<ClaudeModel>, bool)> {
    // The window runs this too, and a window run as administrator runs no CLI.
    crate::elevation::guard().ok()?;
    // Held while it runs, so the sweep does not take its directory meanwhile.
    let (exe, _on_version) = crate::cli::launch(Provider::Claude, None).ok()?;
    // The CLI rewrites the account's `.claude.json` as it starts, from what
    // it read: a trust mark or a setting Luna writes meanwhile would be lost.
    // Luna writes those only under this lock, a file lock the window and the
    // core share (account_settings `editing`), so the ask holds it until the
    // CLI has exited; a write waits out the few seconds of the ask, and the
    // ask a write. The core's account-in-use table (account_use.rs) is not
    // the gate here: this runs in the window, which does not see it, and the
    // trust write the new-chat dialog makes takes the lock, not that table.
    let _editing = match crate::account_settings::editing() {
        Ok(held) => held,
        Err(e) => {
            crate::log::warn("claude-models", &format!("not asking Claude Code for its models: {e}"));
            return None;
        }
    };
    let mut cmd = Command::new(exe);
    cmd.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--strict-mcp-config"])
        .current_dir(account_path)
        .env("CLAUDE_CONFIG_DIR", account_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::pty::clean_env(&mut cmd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().ok()?;
    let request_id = format!("luna-models-{}", crate::throttle::now_ms());
    let mut stdin = child.stdin.take();
    let asked = match stdin.as_mut() {
        Some(s) => writeln!(s, "{}", initialize_request(&request_id)).and_then(|_| s.flush()),
        None => Err(std::io::ErrorKind::BrokenPipe.into()),
    };
    let stdout = child.stdout.take();
    let (tx, rx) = std::sync::mpsc::channel();
    let id = request_id.clone();
    std::thread::spawn(move || {
        let Some(out) = stdout else { return };
        for line in std::io::BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            if line.contains("\"control_response\"") {
                let _ = tx.send(parse_initialize(&line, &id));
                return;
            }
        }
    });
    let answer = asked.ok().and_then(|_| rx.recv_timeout(ASK_TIMEOUT).ok());
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    match answer {
        Some(Some(found)) => Some(found),
        Some(None) => {
            surprise(account_path, "initialize", "the CLI's initialize answer is not in the format Luna knows");
            None
        }
        None => {
            crate::log::warn("claude-models", "Claude Code did not answer initialize in time");
            None
        }
    }
}

// ------------------------------------------------------------ remembered --

#[derive(Serialize, Deserialize, Clone)]
struct Remembered {
    /// When the CLI answered, in ms.
    at: u64,
    models: Vec<ClaudeModel>,
}

/// How long an answer stands before it is asked again, behind.
const REMEMBERED_FOR_MS: u64 = 6 * 3600 * 1000;

/// What the CLI last said per account, in Luna's data folder: an account
/// that has never run since keeps its list across restarts.
fn store_file() -> PathBuf {
    crate::paths::data_dir().join("claude-models.json")
}

fn read_store(file: &Path) -> HashMap<String, Remembered> {
    std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn remember_in(file: &Path, account_path: &str, models: &[ClaudeModel], at: u64) {
    let mut all = read_store(file);
    all.insert(crate::paths::path_key(account_path), Remembered { at, models: models.to_vec() });
    match serde_json::to_vec_pretty(&all) {
        Ok(bytes) => {
            if let Err(e) = crate::secret::write_private(file, &bytes) {
                crate::log::warn("claude-models", &format!("could not keep the model list: {e}"));
            }
        }
        Err(e) => crate::log::warn("claude-models", &format!("could not keep the model list: {e}")),
    }
}

fn store_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

fn remembered(account_path: &str) -> Option<Remembered> {
    let _held = store_lock().lock().unwrap_or_else(|e| e.into_inner());
    read_store(&store_file()).remove(&crate::paths::path_key(account_path))
}

fn remember(account_path: &str, models: &[ClaudeModel]) {
    let _held = store_lock().lock().unwrap_or_else(|e| e.into_inner());
    remember_in(&store_file(), account_path, models, crate::throttle::now_ms());
}

// ------------------------------------------------------------- choosing --

/// What is known of an account's list without running anything, and whether
/// the CLI should be asked behind it.
#[derive(Debug, PartialEq)]
enum Known {
    Current(Vec<ClaudeModel>),
    Stale(Vec<ClaudeModel>),
    Nothing,
}

/// The order of the module doc: a current catalog; a current answer, unless
/// a catalog newer than it says otherwise; a stale catalog; a stale answer.
fn choose(catalog: Option<Catalog>, remembered: Option<Remembered>, now: u64) -> Known {
    let answer_current = remembered.as_ref().is_some_and(|r| now.saturating_sub(r.at) < REMEMBERED_FOR_MS);
    match (catalog, remembered) {
        (Some(c), _) if now < c.stale_at => Known::Current(c.models),
        (c, Some(r)) if answer_current && c.as_ref().is_none_or(|c| r.at > c.fetched_at) => Known::Current(r.models),
        (Some(c), _) => Known::Stale(c.models),
        (None, Some(r)) => Known::Stale(r.models),
        (None, None) => Known::Nothing,
    }
}

fn known(account_path: &str) -> Known {
    choose(read_catalog(account_path), remembered(account_path), crate::throttle::now_ms())
}

/// The list as far as it can be had without running the CLI, the aliases
/// where nothing is known: for checking a spawn and for `luna_session_list`.
pub fn models_known(account_path: &str) -> Vec<ClaudeModel> {
    match known(account_path) {
        Known::Current(m) | Known::Stale(m) => m,
        Known::Nothing => aliases(),
    }
}

/// The accounts whose list the CLI is being asked for now, and when it was
/// last asked: one ask per account at a time, and not again for a few
/// minutes after one that brought nothing.
fn asking() -> &'static Mutex<(HashSet<String>, HashMap<String, Instant>)> {
    static A: OnceLock<Mutex<(HashSet<String>, HashMap<String, Instant>)>> = OnceLock::new();
    A.get_or_init(|| Mutex::new((HashSet::new(), HashMap::new())))
}

const ASK_AGAIN_AFTER: Duration = Duration::from_secs(10 * 60);

/// Asks the CLI unless it is being asked for this account already or was
/// lately; what it says is remembered. None when it was not asked or said
/// nothing usable.
fn ask(account_path: &str) -> Option<Vec<ClaudeModel>> {
    let key = crate::paths::path_key(account_path);
    {
        let mut a = asking().lock().unwrap_or_else(|e| e.into_inner());
        if a.0.contains(&key) || a.1.get(&key).is_some_and(|t| t.elapsed() < ASK_AGAIN_AFTER) {
            return None;
        }
        a.0.insert(key.clone());
        a.1.insert(key.clone(), Instant::now());
    }
    let found = from_cli(account_path);
    asking().lock().unwrap_or_else(|e| e.into_inner()).0.remove(&key);
    let (models, signed_in) = found?;
    // Signed out, the CLI lists what it would offer anyone, not this
    // account: good for this once, not to keep.
    if signed_in {
        remember(account_path, &models);
    }
    Some(models)
}

pub fn models_for(account_path: &str) -> Vec<ClaudeModel> {
    match known(account_path) {
        Known::Current(m) => m,
        Known::Stale(m) => {
            let path = account_path.to_owned();
            std::thread::spawn(move || ask(&path));
            m
        }
        Known::Nothing => ask(account_path).unwrap_or_else(aliases),
    }
}

/// Format surprises, logged once per account and source.
fn surprise(account_path: &str, source: &str, what: &str) {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let key = format!("{source}\n{}", crate::paths::path_key(account_path));
    if SEEN.get_or_init(Default::default).lock().map(|mut s| s.insert(key)).unwrap_or(false) {
        crate::log::warn("claude-models", &format!("{account_path}: {what}"));
    }
}

/// Async and off to a blocking thread: asking the CLI takes seconds, and a
/// sync command would run it on the window's own thread.
///
/// A miss runs Claude Code with `CLAUDE_CONFIG_DIR` set to the account, which
/// reads its settings and credentials and may renew its token there: the
/// page's path is held to an account folder of Luna's (boundary.rs). An empty
/// one, no account chosen yet, lists nothing and runs nothing.
#[tauri::command]
pub async fn claude_models(account_path: OrEmpty<AccountPath>) -> Vec<ClaudeModel> {
    let account_path = account_path.into_string();
    if account_path.is_empty() {
        return vec![];
    }
    tauri::async_runtime::spawn_blocking(move || models_for(&account_path)).await.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOG: &str = include_str!("testdata/model-catalog-2.1.322.json");
    const INITIALIZE: &str = include_str!("testdata/initialize-2.1.322.json");
    const INITIALIZE_SIGNED_IN: &str = include_str!("testdata/initialize-signed-in-2.1.322.json");

    fn catalog() -> Catalog {
        parse_catalog(CATALOG).expect("the 2.1.322 catalog reads")
    }

    fn slugs(m: &[ClaudeModel]) -> Vec<&str> {
        m.iter().map(|m| m.slug.as_str()).collect()
    }

    #[test]
    fn the_catalog_lists_main_models_by_alias_and_the_rest_by_id() {
        let c = catalog();
        assert_eq!(
            slugs(&c.models),
            ["opus", "fable", "sonnet", "haiku", "claude-fable-5-1", "claude-haiku-4-5-20251001", "claude-opus-4-7"]
        );
        let opus = &c.models[0];
        assert_eq!(opus.model_id.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(opus.display_name, "Opus 5.5");
        assert!(!opus.more);
        assert_eq!(opus.efforts, ["low", "medium", "high", "xhigh", "max"]);
        assert_eq!(opus.default_effort.as_deref(), Some("medium"));
        assert_eq!(opus.effort_hints.get("medium").map(String::as_str), Some("Recommended"));
        assert!(opus.effort_hints.get("max").is_some_and(|h| h.contains("3.5")), "a tooltip wins over a badge");
        let older = &c.models[6];
        assert!(older.more);
        assert_eq!(older.display_name, "Opus 4.7");
        assert_eq!(older.default_effort.as_deref(), Some("xhigh"));
    }

    #[test]
    fn a_model_without_thinking_takes_no_effort() {
        let haiku = catalog().models.into_iter().find(|m| m.slug == "claude-haiku-4-5-20251001").unwrap();
        assert!(haiku.efforts.is_empty());
        assert_eq!(haiku.default_effort, None);
    }

    #[test]
    fn the_alias_goes_only_to_the_model_it_targets_in_the_main_section() {
        let mut v: serde_json::Value = serde_json::from_str(CATALOG).unwrap();
        // The alias moves to an older Opus: the main one is named by id now,
        // and the older one, overflow, still by its id.
        v["catalog"]["config"]["provider_alias_targets"]["opus"]["default"] = "claude-opus-4-7".into();
        let c = parse_catalog(&v.to_string()).unwrap();
        assert!(slugs(&c.models).contains(&"claude-opus-5-5"));
        assert!(slugs(&c.models).contains(&"claude-opus-4-7"));
        assert!(!slugs(&c.models).contains(&"opus"));
        // A family Luna has never heard of comes in by its alias all the same.
        let mut v: serde_json::Value = serde_json::from_str(CATALOG).unwrap();
        v["catalog"]["config"]["models"][0]["short_name"] = "Mythos".into();
        v["catalog"]["config"]["provider_alias_targets"]["mythos"] = serde_json::json!({ "default": "claude-opus-5-5" });
        assert_eq!(parse_catalog(&v.to_string()).unwrap().models[0].slug, "mythos");
    }

    #[test]
    fn a_catalog_surprise_skips_the_whole_file() {
        let bad = |f: &dyn Fn(&mut serde_json::Value)| {
            let mut v: serde_json::Value = serde_json::from_str(CATALOG).unwrap();
            f(&mut v);
            parse_catalog(&v.to_string()).is_none()
        };
        assert!(bad(&|v| v["version"] = 3.into()));
        assert!(bad(&|v| v["staleAt"] = "soon".into()));
        assert!(bad(&|v| v["catalog"]["config"]["models"] = serde_json::json!([])));
        assert!(bad(&|v| v["catalog"]["config"]["models"][2]["section"] = "featured".into()));
        assert!(bad(&|v| v["catalog"]["config"]["models"][1]["id"] = serde_json::Value::Null));
        assert!(bad(&|v| v["catalog"]["config"]["models"][0]["runtime"]["effort_levels"] = serde_json::json!(["low", 5])));
        assert!(bad(&|v| v["catalog"]["config"]["models"][0]["thinking"] = serde_json::json!({})));
        assert!(bad(&|v| v["catalog"]["config"]["provider_alias_targets"] = serde_json::Value::Null));
        assert!(parse_catalog("{not json").is_none());
    }

    #[test]
    fn the_initialize_answer_lists_the_aliases_with_their_levels() {
        // The fixture is pretty-printed; the CLI writes it as one line.
        let line = serde_json::from_str::<serde_json::Value>(INITIALIZE).unwrap().to_string();
        let (m, signed_in) = parse_initialize(&line, "r1").unwrap();
        assert!(!signed_in, "recorded on a signed-out throwaway account");
        assert_eq!(slugs(&m), ["opus", "fable", "sonnet", "haiku"], "no `default` entry");
        assert_eq!(m[0].model_id.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(m[0].display_name, "Opus");
        assert_eq!(m[0].efforts, ["low", "medium", "high", "xhigh", "max"]);
        assert!(m.iter().all(|m| !m.more));
    }

    #[test]
    fn a_signed_in_answer_is_the_accounts_and_lists_the_same_aliases() {
        // Recorded on a signed-in clone (its email taken out): the same four
        // aliases, no older models, and an account with no `tokenSource`.
        let line = serde_json::from_str::<serde_json::Value>(INITIALIZE_SIGNED_IN).unwrap().to_string();
        let (m, signed_in) = parse_initialize(&line, "r1").unwrap();
        assert!(signed_in);
        assert_eq!(slugs(&m), ["opus", "fable", "sonnet", "haiku"]);
        assert_eq!(m[3].model_id.as_deref(), Some("claude-haiku-5-5"));
    }

    #[test]
    fn an_initialize_answer_out_of_shape_is_not_taken() {
        let line = |models: serde_json::Value| {
            serde_json::json!({ "type": "control_response", "response": { "subtype": "success", "request_id": "r1",
                "response": { "models": models, "account": { "subscriptionType": "Claude Max", "apiProvider": "firstParty" } } } })
            .to_string()
        };
        let ok = line(serde_json::json!([
            { "value": "opus", "displayName": "Opus", "supportsEffort": true, "supportedEffortLevels": ["low", "high"] },
            { "value": "claude-haiku-4-5-20251001", "displayName": "Haiku 4.5", "supportsEffort": false }
        ]));
        let (m, signed_in) = parse_initialize(&ok, "r1").unwrap();
        assert!(signed_in);
        assert!(m[1].more && m[1].efforts.is_empty());
        assert!(parse_initialize(&ok, "r2").is_none(), "another request's answer");
        assert!(parse_initialize(&line(serde_json::json!([{ "value": "opus" }])), "r1").is_none());
        assert!(parse_initialize(&line(serde_json::json!({ "opus": {} })), "r1").is_none());
        assert!(parse_initialize(
            &line(serde_json::json!([{ "value": "opus", "displayName": "Opus", "supportedEffortLevels": "high" }])),
            "r1"
        )
        .is_none());
        assert!(parse_initialize(&line(serde_json::json!([])), "r1").is_none());
    }

    #[test]
    fn the_request_is_the_sdks_first_line() {
        let v: serde_json::Value = serde_json::from_str(&initialize_request("x")).unwrap();
        assert_eq!(v, serde_json::json!({ "type": "control_request", "request_id": "x", "request": { "subtype": "initialize" } }));
    }

    #[test]
    fn a_stale_catalog_still_answers_and_a_newer_answer_wins_over_it() {
        let stale_at = catalog().stale_at;
        let fetched = catalog().fetched_at;
        let answer = |at| Remembered { at, models: aliases() };
        assert!(matches!(choose(Some(catalog()), None, stale_at - 1), Known::Current(m) if m[0].model_id.is_some()));
        assert!(matches!(choose(Some(catalog()), None, stale_at), Known::Stale(m) if m[0].model_id.is_some()));
        // Asked after the catalog was written, and lately: the answer.
        assert_eq!(choose(Some(catalog()), Some(answer(stale_at + 10)), stale_at + 20), Known::Current(aliases()));
        // Asked before it: the catalog, stale as it is.
        assert!(matches!(choose(Some(catalog()), Some(answer(fetched - 10)), stale_at + 20), Known::Stale(m) if m[0].model_id.is_some()));
        // No catalog: the answer, current for a while, then stale.
        assert_eq!(choose(None, Some(answer(1000)), 1000 + REMEMBERED_FOR_MS - 1), Known::Current(aliases()));
        assert_eq!(choose(None, Some(answer(1000)), 1000 + REMEMBERED_FOR_MS), Known::Stale(aliases()));
        assert_eq!(choose(None, None, 0), Known::Nothing);
    }

    #[test]
    fn the_catalog_is_read_from_the_accounts_cache_and_a_bad_one_is_skipped() {
        let root = std::env::temp_dir().join(format!("luna-claude-models-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("cache").join("model-catalog");
        std::fs::create_dir_all(&dir).unwrap();
        let account = root.to_string_lossy().into_owned();
        assert!(read_catalog(&account).is_none());
        std::fs::write(dir.join("org-user-cc.json"), CATALOG).unwrap();
        assert_eq!(read_catalog(&account).unwrap().models.len(), 7);
        std::fs::write(dir.join("org-user-cc.json"), r#"{"version":9}"#).unwrap();
        assert!(read_catalog(&account).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn answers_are_kept_per_account() {
        let root = std::env::temp_dir().join(format!("luna-claude-models-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("claude-models.json");
        remember_in(&file, "/a/one", &aliases()[..1], 5);
        remember_in(&file, "/a/two", &aliases(), 6);
        let all = read_store(&file);
        assert_eq!(all[&crate::paths::path_key("/a/one")].models.len(), 1);
        assert_eq!(all[&crate::paths::path_key("/a/two")].at, 6);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_fallback_is_the_four_aliases() {
        assert_eq!(slugs(&aliases()), ALIASES);
        assert!(aliases().iter().all(|m| m.efforts.len() == STOCK_EFFORTS.len() && !m.more));
    }
}
