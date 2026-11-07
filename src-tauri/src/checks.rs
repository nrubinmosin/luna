//! What Luna asks of its own settings, the accounts' files and the machine for
//! its features to work, said out loud: a feature that goes quiet because of a
//! setting should say so here rather than in a line of the log.
//!
//! Each check reads one thing and says what depends on it. Where one click can
//! put it right the check names the fix; every fix is asked for by id and
//! worked out here, from the files as they are at that moment, so the page
//! cannot name a key. A fix that does more than the row's title — takes a
//! barrier away, reaches other accounts — carries a `risk` the page has to
//! have shown and had confirmed. Nothing is ever applied unasked. The keys that
//! are a matter of taste stay in `account_settings.rs`.
//!
//! All of it runs in the core: only the core knows which accounts are in use
//! (account_use.rs).

use crate::account_settings::{edit_together, Doc, Edit, KeyFile};
use crate::provider::Provider;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Ok,
    /// Something Luna relies on is off, and `action` may put it right.
    Off,
    /// Something outside Luna's reach has it off: no fix to offer.
    Blocked,
    /// What it depends on would not read.
    Unknown,
}

/// One check as the Settings dialog shows it.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub id: String,
    /// `provider/name`; None for what holds for the whole machine.
    pub account: Option<String>,
    pub title: String,
    /// What goes wrong without it.
    pub why: String,
    pub status: Status,
    /// What it says now, and what it would say.
    pub current: Option<String>,
    pub target: Option<String>,
    /// To show and have confirmed before `action`.
    pub risk: Option<String>,
    pub note: Option<String>,
    /// The button; None with nothing to do.
    pub action: Option<String>,
    /// Why the button cannot be pressed just now.
    pub refused: Option<String>,
    pub dismissed: bool,
}

// ------------------------------------------------------------------- facts --

/// A policy file's word on hooks.
#[derive(Default, Clone)]
struct Policy {
    /// The key that keeps Luna's hooks from running.
    blocked_by: Option<String>,
    /// A file that would not read, which may say anything.
    unreadable: Option<String>,
}

struct Account {
    provider: Provider,
    name: String,
    path: String,
    /// A session of Luna's on it, one on its way out included.
    in_use: bool,
    /// The account's settings file.
    main: Result<Doc, String>,
    /// Claude Code's `.claude.json`.
    global: Result<Doc, String>,
}

impl Account {
    fn doc(&self, file: KeyFile) -> &Result<Doc, String> {
        if file == KeyFile::ClaudeGlobal {
            &self.global
        } else {
            &self.main
        }
    }

    fn label(&self) -> String {
        format!("{}/{}", self.provider.as_str(), self.name)
    }
}

struct Facts {
    settings: crate::settings::Settings,
    claude_policy: Policy,
    codex_policy: Policy,
    claude_version: Option<String>,
    /// What the machine's Codex defaults say of the hooks feature.
    codex_default_hooks: Option<bool>,
    accounts: Vec<Account>,
}

/// What a check of one file goes by beside the file.
#[derive(Clone, Copy)]
struct Ctx<'a> {
    claude_version: Option<&'a str>,
    codex_default_hooks: Option<bool>,
}

impl Facts {
    fn ctx(&self) -> Ctx<'_> {
        Ctx { claude_version: self.claude_version.as_deref(), codex_default_hooks: self.codex_default_hooks }
    }

    fn parent_name(&self, provider: Provider) -> Option<&str> {
        match provider {
            Provider::Claude => self.settings.settings_parents.claude.as_deref(),
            Provider::Codex => self.settings.settings_parents.codex.as_deref(),
        }
    }

    fn parent_of(&self, a: &Account) -> Option<&Account> {
        let name = self.parent_name(a.provider)?;
        self.accounts.iter().find(|p| p.provider == a.provider && p.name == name)
    }

    fn has(&self, provider: Provider) -> bool {
        self.accounts.iter().any(|a| a.provider == provider)
    }
}

/// Later files in the CLI's order have the last word on a key, as `.d` drop-ins
/// have over `managed-settings.json`: base first, then the folder's `*.json`
/// sorted by name, dotfiles left out.
fn claude_policy_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = vec![dir.join("managed-settings.json")];
    if let Ok(entries) = std::fs::read_dir(dir.join("managed-settings.d")) {
        let mut names: Vec<_> = entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".json") && !n.starts_with('.'))
            .collect();
        names.sort();
        files.extend(names.into_iter().map(|n| dir.join("managed-settings.d").join(n)));
    }
    files
}

/// `disableAllHooks` runs no hook at all and `allowManagedHooksOnly` only the
/// policy's own: Luna's come from `--settings`, so either ends them, in Claude
/// Code 2.1.318 ahead of any account's say. MDM and server-managed policy are
/// not read here.
fn claude_policy(dir: &Path) -> Policy {
    let (mut disabled, mut managed_only) = (None, None);
    let mut unreadable = None;
    for path in claude_policy_files(dir) {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                unreadable = Some(format!("{}: {e}", path.display()));
                continue;
            }
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(v) if v.is_object() => {
                disabled = v["disableAllHooks"].as_bool().or(disabled);
                managed_only = v["allowManagedHooksOnly"].as_bool().or(managed_only);
            }
            Ok(_) => unreadable = Some(format!("{}: not a JSON object", path.display())),
            Err(e) => unreadable = Some(format!("{}: {e}", path.display())),
        }
    }
    let blocked_by = if disabled == Some(true) {
        Some("disableAllHooks".to_string())
    } else if managed_only == Some(true) {
        Some("allowManagedHooksOnly".to_string())
    } else {
        None
    };
    Policy { blocked_by, unreadable }
}

/// Where Codex 0.166 keeps a machine's own: `requirements.toml`, which an
/// administrator enforces, and `config.toml`, defaults an account can override.
fn codex_system_dir() -> PathBuf {
    if cfg!(windows) {
        let root = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into());
        Path::new(&root).join("OpenAI").join("Codex")
    } else {
        PathBuf::from("/etc/codex")
    }
}

/// What the machine's Codex defaults say of the hooks feature, and None with no
/// file or no word in it. An account's own key goes over this.
fn codex_system_hooks(path: &Path) -> Result<Option<bool>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let doc = text.parse::<toml_edit::DocumentMut>().map_err(|e| format!("{}: {e}", path.display()))?;
            Ok(hooks_said(&Doc::Toml(doc)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// `allow_managed_hooks_only` keeps every hook but the administrator's from
/// running, Luna's among them, and a requirement can pin the feature off.
fn codex_policy(path: &Path) -> Policy {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Policy::default(),
        Err(e) => return Policy { blocked_by: None, unreadable: Some(format!("{}: {e}", path.display())) },
    };
    let doc = match text.parse::<toml_edit::DocumentMut>() {
        Ok(doc) => doc,
        Err(e) => return Policy { blocked_by: None, unreadable: Some(format!("{}: {e}", path.display())) },
    };
    let doc = Doc::Toml(doc);
    let pinned_off =
        ["features", "feature_requirements"].into_iter().any(|table| HOOK_FEATURES.iter().any(|f| doc.bool_at(&[table, f]) == Some(false)));
    let blocked_by = if doc.bool_at(&["allow_managed_hooks_only"]) == Some(true) {
        Some("allow_managed_hooks_only".to_string())
    } else if pinned_off {
        Some("features.hooks".to_string())
    } else {
        None
    };
    Policy { blocked_by, unreadable: None }
}

fn gather() -> Result<Facts, String> {
    let settings = crate::settings::get_checked()?;
    let accounts = crate::accounts::list_accounts()?
        .into_iter()
        .map(|a| {
            let (main, global) = match a.provider {
                Provider::Claude => (KeyFile::ClaudeSettings.read(&a.path), KeyFile::ClaudeGlobal.read(&a.path)),
                Provider::Codex => (KeyFile::CodexConfig.read(&a.path), Ok(Doc::Json(json!({})))),
            };
            let in_use = crate::pty::manager().account_in_use(&a.path);
            Account { provider: a.provider, name: a.name, path: a.path, in_use, main, global }
        })
        .collect();
    let mut codex_policy = codex_policy(&codex_system_dir().join("requirements.toml"));
    let codex_default_hooks = codex_system_hooks(&codex_system_dir().join("config.toml")).unwrap_or_else(|e| {
        codex_policy.unreadable.get_or_insert(e);
        None
    });
    Ok(Facts {
        settings,
        claude_policy: claude_policy(&crate::claude::defaults::managed_dir()),
        codex_policy,
        claude_version: crate::cli::current_version(Provider::Claude),
        codex_default_hooks,
        accounts,
    })
}

// ------------------------------------------------------------ account checks --

/// A check on one key of one account file.
struct Spec {
    id: &'static str,
    provider: Provider,
    file: KeyFile,
    title: &'static str,
    why: &'static str,
    action: &'static str,
    target: fn(Ctx) -> &'static str,
    /// What holds with this file in place, when that is the trouble.
    off: fn(&Doc, Ctx) -> Option<String>,
    edits: fn(Ctx) -> Vec<Edit>,
    /// Not to be written while a session of Luna's is on the account: the CLI
    /// rewrites this file from what it read.
    quiet: bool,
    /// Whatever this fix does beyond the row's title, this account's sync aside.
    risk: fn(&Facts) -> Option<String>,
    /// The account has chosen otherwise, and there is nothing to say.
    moot: fn(&Account) -> bool,
}

/// `hooks` and the older name Codex 0.166 still takes for it.
const HOOK_FEATURES: [&str; 2] = ["hooks", "codex_hooks"];

fn claude_hooks_off(doc: &Doc, _: Ctx) -> Option<String> {
    (doc.bool_at(&["disableAllHooks"]) == Some(true)).then(|| "disableAllHooks: true".to_string())
}

fn consent_off(doc: &Doc, _: Ctx) -> Option<String> {
    let path = ["skipDangerousModePermissionPrompt"];
    (doc.bool_at(&path) != Some(true)).then(|| doc.said(&path).unwrap_or_else(|| "not set".into()))
}

/// Turned off after repeated failed starts. Claude Code keeps the mark per
/// version and reads it only for its own, so one left by an older CLI is no
/// trouble. With no CLI to ask, it counts.
fn fullscreen_off(doc: &Doc, ctx: Ctx) -> Option<String> {
    let path = ["fullscreenAutoDisabled", "version"];
    let said = doc.said(&["fullscreenAutoDisabled"])?;
    let of = doc.said(&path).map(|v| v.trim_matches('"').to_string());
    match (of, ctx.claude_version) {
        (Some(of), Some(version)) if of != version => None,
        (Some(of), _) => Some(format!("turned off on {of}")),
        (None, _) => Some(said),
    }
}

/// What one file says of the feature. Off under either name counts.
fn hooks_said(doc: &Doc) -> Option<bool> {
    let said: Vec<bool> = HOOK_FEATURES.iter().filter_map(|f| doc.bool_at(&["features", f])).collect();
    said.iter().copied().find(|on| !on).or(said.first().copied())
}

/// The feature as Codex ends up with it: the account's word if it has one, the
/// machine's defaults if not.
fn codex_hooks_off(doc: &Doc, ctx: Ctx) -> Option<String> {
    match hooks_said(doc).or(ctx.codex_default_hooks) {
        Some(false) if hooks_said(doc).is_some() => {
            HOOK_FEATURES.iter().find(|f| doc.bool_at(&["features", f]) == Some(false)).map(|f| format!("features.{f} = false"))
        }
        Some(false) => Some("off by this computer's Codex defaults".to_string()),
        _ => None,
    }
}

fn no_risk(_: &Facts) -> Option<String> {
    None
}

fn never(_: &Account) -> bool {
    false
}

fn specs() -> Vec<Spec> {
    vec![
        Spec {
            id: "claude-hooks",
            provider: Provider::Claude,
            file: KeyFile::ClaudeSettings,
            title: "Hooks are switched off",
            why: "Claude Code runs no hook on this account, and Luna's are how it hears of a turn's end, a question and a subagent; \
                  the guard against endless waits goes with them.",
            action: "Turn hooks on",
            target: |_| "key removed",
            off: claude_hooks_off,
            edits: |_| vec![Edit::RemoveIf(&["disableAllHooks"], true)],
            quiet: false,
            risk: |_| Some("Every hook of this account comes back, not only Luna's: any written in its settings that you have not reviewed runs at its next event.".into()),
            moot: never,
        },
        Spec {
            id: "claude-bypass-consent",
            provider: Provider::Claude,
            file: KeyFile::ClaudeSettings,
            title: "Bypass is not accepted",
            why: "A chat in Bypass, which is how new chats start, stops on Claude Code's warning until someone answers it.",
            action: "Accept Bypass",
            target: |_| "true",
            off: consent_off,
            edits: |_| vec![Edit::Set(&["skipDangerousModePermissionPrompt"], json!(true))],
            quiet: false,
            risk: |_| Some("This records your acceptance of Claude Code's warning for this account: it will not ask again before running in Bypass.".into()),
            moot: never,
        },
        Spec {
            id: "claude-fullscreen",
            provider: Provider::Claude,
            file: KeyFile::ClaudeGlobal,
            title: "Fullscreen was turned off",
            why: "Claude Code gave up on its fullscreen renderer here after launches that did not finish, so panes flicker and redraw badly on resize.",
            action: "Try fullscreen again",
            target: |_| "cleared",
            off: fullscreen_off,
            edits: |_| vec![Edit::Remove(&["fullscreenAutoDisabled"]), Edit::Remove(&["fullscreenBootStrikes"])],
            quiet: true,
            risk: no_risk,
            // The classic renderer asked for by name.
            moot: |a| matches!(&a.main, Ok(doc) if doc.said(&["tui"]).as_deref() == Some("\"default\"")),
        },
        Spec {
            id: "codex-hooks-feature",
            provider: Provider::Codex,
            file: KeyFile::CodexConfig,
            title: "Hooks are switched off",
            why: "Codex runs no hook on this account, so Luna's guard against endless waits never does.",
            action: "Turn hooks on",
            target: |ctx| if ctx.codex_default_hooks == Some(false) { "features.hooks = true" } else { "key removed" },
            off: codex_hooks_off,
            // Taking the key out would leave the machine's default standing.
            edits: |ctx| {
                let mut edits: Vec<Edit> = HOOK_FEATURE_KEYS.into_iter().map(|path| Edit::RemoveIf(path, false)).collect();
                if ctx.codex_default_hooks == Some(false) {
                    edits.push(Edit::Set(HOOK_FEATURE_KEYS[0], json!(true)));
                }
                edits
            },
            quiet: true,
            risk: |f| {
                Some(format!(
                    "Every enabled hook of this account comes back, not only Luna's.{}",
                    if f.settings.codex_hooks_unasked { " With Codex hooks without asking on, those of the projects it opens run with no review." } else { "" }
                ))
            },
            moot: never,
        },
    ]
}

const HOOK_FEATURE_KEYS: [&[&str]; 2] = [&["features", "hooks"], &["features", "codex_hooks"]];

fn off_or_unknown(a: &Account, spec: &Spec, f: &Facts) -> bool {
    match a.doc(spec.file) {
        Ok(doc) => (spec.off)(doc, f.ctx()).is_some(),
        Err(_) => true,
    }
}

/// The accounts a fix may have to write. A sync copies the parent's whole file,
/// so a fix left off a parent that says the same would be undone by the next
/// one; whether it does is for the moment of writing to say. `.claude.json`
/// does not travel.
fn candidates<'a>(f: &'a Facts, a: &'a Account, spec: &Spec) -> Vec<&'a Account> {
    let mut out = vec![a];
    if spec.file != KeyFile::ClaudeGlobal {
        out.extend(f.parent_of(a).filter(|p| p.name != a.name));
    }
    out
}

/// What a sync makes of this fix, for an account with a parent.
fn sync_effect(f: &Facts, a: &Account, spec: &Spec) -> Option<String> {
    if spec.file == KeyFile::ClaudeGlobal {
        return None;
    }
    let parent = f.parent_of(a)?;
    let others: Vec<&str> = f
        .accounts
        .iter()
        .filter(|o| o.provider == a.provider && o.name != parent.name && o.name != a.name)
        .map(|o| o.name.as_str())
        .collect();
    let reach = if others.is_empty() { None } else { Some(format!(" The next settings sync copies it to {}.", others.join(", "))) };
    if parent.name == a.name {
        return Some(format!("This is the accounts' settings parent.{}", reach.unwrap_or_default()));
    }
    off_or_unknown(parent, spec, f)
        .then(|| format!("Its settings parent {} says the same and is changed with it.{}", parent.name, reach.unwrap_or_default()))
}

fn account_check(f: &Facts, a: &Account, spec: &Spec) -> Check {
    let mut c = Check {
        id: spec.id.into(),
        account: Some(a.label()),
        title: spec.title.into(),
        why: spec.why.into(),
        status: Status::Ok,
        current: None,
        target: None,
        risk: None,
        note: None,
        action: None,
        refused: None,
        dismissed: false,
    };
    let doc = match a.doc(spec.file) {
        Ok(doc) => doc,
        Err(e) => {
            c.status = Status::Unknown;
            c.note = Some(e.clone());
            return c;
        }
    };
    let Some(current) = (spec.off)(doc, f.ctx()).filter(|_| !(spec.moot)(a)) else {
        return c;
    };
    c.status = Status::Off;
    c.current = Some(current);
    c.target = Some((spec.target)(f.ctx()).into());
    c.action = Some(spec.action.into());
    let parts: Vec<String> = [(spec.risk)(f), sync_effect(f, a, spec)].into_iter().flatten().collect();
    c.risk = (!parts.is_empty()).then(|| parts.join(" "));
    if spec.quiet {
        let name = if a.provider == Provider::Claude { "Claude Code" } else { "Codex" };
        c.note = Some(format!(
            "Written only while no session of Luna's is on the account. A {name} running outside Luna can still race this write."
        ));
        if let Some(busy) = candidates(f, a, spec).into_iter().find(|t| t.in_use) {
            c.refused = Some(format!("{} is in use. Stop its sessions; one closing counts until its process has exited.", busy.name));
        }
    }
    c
}

// ------------------------------------------------------------ machine checks --

const UNASKED_RISK: &str = "This runs every project's hooks without review in every Codex chat and helper, not only Luna's guard: \
    a repository you open can run what it likes on this computer.";

/// A check of Luna's own settings, with its fix.
struct Own {
    id: &'static str,
    title: &'static str,
    why: &'static str,
    action: &'static str,
    ok: fn(&crate::settings::Settings) -> bool,
    fix: fn(&mut crate::settings::Settings),
    risk: Option<&'static str>,
    needs: Option<Provider>,
}

fn own() -> Vec<Own> {
    vec![
        Own {
            id: "codex-hooks-unasked",
            title: "Codex hooks without asking is off",
            why: "Codex drops a hook it has not been told to trust, so Luna's guard against endless waits never reaches Codex chats and helpers.",
            action: "Turn on",
            ok: |s| s.codex_hooks_unasked,
            fix: |s| s.codex_hooks_unasked = true,
            risk: Some(UNASKED_RISK),
            needs: Some(Provider::Codex),
        },
        Own {
            id: "keep-awake",
            title: "Keep awake is off",
            why: "The PC can go to sleep under a running chat or helper, and the work stops with it.",
            action: "Turn on",
            ok: |s| s.keep_awake,
            fix: |s| s.keep_awake = true,
            risk: None,
            needs: None,
        },
        Own {
            id: "waiting-marker",
            title: "Waiting for you is off",
            why: "A chat that ends its turn on the marker reads as idle, not as waiting for you.",
            action: "Turn on",
            ok: |s| s.waiting_marker_on && !s.waiting_marker.trim().is_empty(),
            fix: |s| {
                s.waiting_marker_on = true;
                if s.waiting_marker.trim().is_empty() {
                    s.waiting_marker = crate::settings::Settings::default().waiting_marker;
                }
            },
            risk: None,
            needs: None,
        },
        Own {
            id: "fetch-before-worktree",
            title: "Fetch before a worktree is off",
            why: "Claude Code branches a worktree off origin as last fetched, which can be a day old.",
            action: "Turn on",
            ok: |s| s.fetch_before_worktree,
            fix: |s| s.fetch_before_worktree = true,
            risk: None,
            needs: None,
        },
    ]
}

fn own_check(f: &Facts, o: &Own) -> Check {
    let ok = (o.ok)(&f.settings);
    // A folder's own choice stands: it is told of, never changed from here.
    let kept_off: Vec<&str> = if o.id == "codex-hooks-unasked" {
        f.settings.codex_hooks_unasked_in.iter().filter(|(_, on)| !**on).map(|(path, _)| path.as_str()).collect()
    } else {
        Vec::new()
    };
    Check {
        id: o.id.into(),
        account: None,
        title: o.title.into(),
        why: o.why.into(),
        status: if ok { Status::Ok } else { Status::Off },
        current: (!ok).then(|| "off".to_string()),
        target: (!ok).then(|| "on".to_string()),
        risk: o.risk.filter(|_| !ok).map(str::to_string),
        note: (!kept_off.is_empty()).then(|| format!("Kept off for these folders, which keep their own choice: {}", kept_off.join(", "))),
        action: (!ok).then(|| o.action.to_string()),
        refused: None,
        dismissed: false,
    }
}

fn policy_check(id: &str, title: &str, cli: &str, policy: &Policy) -> Option<Check> {
    let (status, current, note) = match (&policy.blocked_by, &policy.unreadable) {
        (Some(key), _) => (
            Status::Blocked,
            Some(key.clone()),
            Some("Set by your administrator: nothing in Luna or an account can change it.".to_string()),
        ),
        (None, Some(e)) => (Status::Unknown, None, Some(e.clone())),
        (None, None) => return None,
    };
    Some(Check {
        id: id.into(),
        account: None,
        title: title.into(),
        why: format!("A policy on this computer keeps {cli} from running Luna's hooks, on every account."),
        status,
        current,
        target: None,
        risk: None,
        note,
        action: None,
        refused: None,
        dismissed: false,
    })
}

fn dismiss_key(id: &str, account: Option<&str>) -> String {
    match account {
        Some(account) => format!("{id}@{account}"),
        None => id.to_string(),
    }
}

fn evaluate(f: &Facts) -> Vec<Check> {
    let mut out = Vec::new();
    if f.has(Provider::Claude) {
        out.extend(policy_check("claude-policy-hooks", "Hooks are limited by policy", "Claude Code", &f.claude_policy));
    }
    if f.has(Provider::Codex) {
        out.extend(policy_check("codex-policy-hooks", "Hooks are limited by policy", "Codex", &f.codex_policy));
    }
    for o in own().iter().filter(|o| o.needs.is_none_or(|p| f.has(p))) {
        out.push(own_check(f, o));
    }
    for spec in specs() {
        for a in f.accounts.iter().filter(|a| a.provider == spec.provider) {
            out.push(account_check(f, a, &spec));
        }
    }
    for c in &mut out {
        c.dismissed = f.settings.recommendations_dismissed.contains(&dismiss_key(&c.id, c.account.as_deref()));
    }
    out
}

// -------------------------------------------------------------------- apply --

/// Puts one check right, from what the files say now. What only needs Luna's
/// own settings changes them; what needs an account's file goes in as one change
/// across the account and its parent.
fn apply(f: &Facts, id: &str, account: Option<&str>, confirmed: bool) -> Result<(), String> {
    let list = evaluate(f);
    let c = list.iter().find(|c| c.id == id && c.account.as_deref() == account).ok_or("No such recommendation")?;
    match c.status {
        Status::Ok => return Ok(()),
        Status::Off => {}
        Status::Blocked => return Err("Nothing in Luna can change this".into()),
        Status::Unknown => return Err(c.note.clone().unwrap_or_else(|| "Could not read what this depends on".into())),
    }
    if c.risk.is_some() && !confirmed {
        return Err("This one has to be confirmed first".into());
    }
    if let Some(busy) = &c.refused {
        return Err(busy.clone());
    }
    let Some(account) = account else {
        let o = own().into_iter().find(|o| o.id == id).ok_or("No such recommendation")?;
        // The power module has a hold of its own to bring in line.
        if id == "keep-awake" {
            crate::power::set_keep_awake(true)?;
            return Ok(());
        }
        return crate::settings::update(o.fix);
    };
    let spec = specs().into_iter().find(|s| s.id == id).ok_or("No such recommendation")?;
    let a = f.accounts.iter().find(|a| a.label() == account).ok_or("No such account")?;
    let candidates = candidates(f, a, &spec);
    let paths: Vec<String> = candidates.iter().map(|t| t.path.clone()).collect();
    // What the row told of, and so what the yes covers.
    let shown: Vec<String> =
        candidates.iter().filter(|t| t.name == a.name || off_or_unknown(t, &spec, f)).map(|t| t.path.clone()).collect();
    // First the hold, then the file lock, as a sync takes them; `refused` above
    // only said what was so a moment ago.
    let held = if spec.quiet { Some(crate::account_use::quiet(&paths)?) } else { None };
    let ctx = f.ctx();
    let changed = edit_together(spec.file, &paths, &shown, &(spec.edits)(ctx), &|doc| (spec.off)(doc, ctx).is_some())?;
    drop(held);
    crate::log::info(
        "settings",
        &format!("recommendation {id} on {account}: {changed} file(s), was {}", c.current.as_deref().unwrap_or("?")),
    );
    Ok(())
}

/// Every check, each with its state now.
pub fn recommendations() -> Result<Vec<Check>, String> {
    Ok(evaluate(&gather()?))
}

pub fn apply_recommendation(id: String, account: Option<String>, confirmed: bool) -> Result<Vec<Check>, String> {
    apply(&gather()?, &id, account.as_deref(), confirmed)?;
    recommendations()
}

/// Leaves one off on purpose, or asks for it back.
pub fn dismiss_recommendation(id: String, account: Option<String>, dismissed: bool) -> Result<Vec<Check>, String> {
    let key = dismiss_key(&id, account.as_deref());
    crate::settings::update(|s| {
        s.recommendations_dismissed.retain(|k| *k != key);
        if dismissed {
            s.recommendations_dismissed.push(key);
        }
    })?;
    recommendations()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("luna-checks-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Accounts in folders of their own under `root`, read as `gather` reads them.
    fn account(root: &Path, provider: Provider, name: &str, files: &[(&str, &str)]) -> Account {
        let d = root.join(provider.as_str()).join(name);
        std::fs::create_dir_all(&d).unwrap();
        for (file, text) in files {
            std::fs::write(d.join(file), text).unwrap();
        }
        let path = d.to_string_lossy().into_owned();
        let (main, global) = match provider {
            Provider::Claude => (KeyFile::ClaudeSettings.read(&path), KeyFile::ClaudeGlobal.read(&path)),
            Provider::Codex => (KeyFile::CodexConfig.read(&path), Ok(Doc::Json(json!({})))),
        };
        Account { provider, name: name.into(), path, in_use: false, main, global }
    }

    fn facts(accounts: Vec<Account>) -> Facts {
        Facts {
            settings: Settings::default(),
            claude_policy: Policy::default(),
            codex_policy: Policy::default(),
            claude_version: Some("2.1.318".into()),
            codex_default_hooks: None,
            accounts,
        }
    }

    fn check<'a>(list: &'a [Check], id: &str, account: Option<&str>) -> &'a Check {
        list.iter().find(|c| c.id == id && c.account.as_deref() == account).unwrap_or_else(|| panic!("no {id} {account:?}"))
    }

    fn read(path: &str, file: &str) -> String {
        std::fs::read_to_string(Path::new(path).join(file)).unwrap()
    }

    #[test]
    fn a_policy_is_read_where_claude_code_reads_it_and_nothing_is_offered_against_it() {
        if cfg!(windows) {
            assert_eq!(crate::claude::defaults::managed_dir(), PathBuf::from(r"C:\Program Files\ClaudeCode"));
        }
        let root = dir("policy");
        let good = root.join("good");
        std::fs::create_dir_all(good.join("managed-settings.d")).unwrap();
        // Healthy account, and a policy that ends its hooks all the same.
        let a = account(&root, Provider::Claude, "fine", &[("settings.json", r#"{"skipDangerousModePermissionPrompt":true}"#)]);
        let mut f = facts(vec![a]);
        std::fs::write(good.join("managed-settings.json"), r#"{"disableAllHooks":true}"#).unwrap();
        f.claude_policy = claude_policy(&good);
        let list = evaluate(&f);
        let c = check(&list, "claude-policy-hooks", None);
        assert_eq!((c.status, c.current.as_deref()), (Status::Blocked, Some("disableAllHooks")));
        assert!(c.action.is_none() && c.target.is_none());
        assert_eq!(check(&list, "claude-hooks", Some("claude/fine")).status, Status::Ok, "the account has nothing to fix");
        assert!(apply(&f, "claude-policy-hooks", None, true).unwrap_err().contains("Nothing in Luna"));

        std::fs::write(good.join("managed-settings.json"), r#"{"allowManagedHooksOnly":true}"#).unwrap();
        assert_eq!(claude_policy(&good).blocked_by.as_deref(), Some("allowManagedHooksOnly"));

        // A drop-in has the last word on a key, in the order of its name.
        std::fs::write(good.join("managed-settings.json"), r#"{"disableAllHooks":true}"#).unwrap();
        std::fs::write(good.join("managed-settings.d").join("10-a.json"), r#"{"disableAllHooks":false}"#).unwrap();
        std::fs::write(good.join("managed-settings.d").join(".hidden.json"), r#"{"disableAllHooks":true}"#).unwrap();
        assert_eq!(claude_policy(&good).blocked_by, None);
        std::fs::write(good.join("managed-settings.d").join("20-b.json"), r#"{"allowManagedHooksOnly":true}"#).unwrap();
        assert_eq!(claude_policy(&good).blocked_by.as_deref(), Some("allowManagedHooksOnly"));

        // Unreadable says nothing either way, and is said to.
        std::fs::remove_dir_all(good.join("managed-settings.d")).unwrap();
        std::fs::write(good.join("managed-settings.json"), "{ not json").unwrap();
        f.claude_policy = claude_policy(&good);
        assert_eq!(check(&evaluate(&f), "claude-policy-hooks", None).status, Status::Unknown);

        // No file, no row.
        f.claude_policy = claude_policy(&root.join("none"));
        assert!(!evaluate(&f).iter().any(|c| c.id == "claude-policy-hooks"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_codex_policy_blocks_the_same_way() {
        let root = dir("policy-codex");
        let path = root.join("requirements.toml");
        assert_eq!(codex_policy(&path).blocked_by, None);
        std::fs::write(&path, "allow_managed_hooks_only = true\n").unwrap();
        assert_eq!(codex_policy(&path).blocked_by.as_deref(), Some("allow_managed_hooks_only"));
        std::fs::write(&path, "[features]\nhooks = false\n").unwrap();
        assert_eq!(codex_policy(&path).blocked_by.as_deref(), Some("features.hooks"));
        std::fs::write(&path, "x = [").unwrap();
        assert!(codex_policy(&path).unreadable.is_some());
        let mut f = facts(vec![account(&root, Provider::Codex, "one", &[])]);
        std::fs::write(&path, "allow_managed_hooks_only = true\n").unwrap();
        f.codex_policy = codex_policy(&path);
        let list = evaluate(&f);
        let c = check(&list, "codex-policy-hooks", None);
        assert!(c.status == Status::Blocked && c.action.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lunas_own_checks_say_what_is_off_and_keep_a_folders_choice() {
        let mut f = facts(vec![]);
        let list = evaluate(&f);
        assert!(!list.iter().any(|c| c.id == "codex-hooks-unasked"), "no Codex account, no Codex check");
        for id in ["keep-awake", "waiting-marker", "fetch-before-worktree"] {
            assert_eq!(check(&list, id, None).status, Status::Ok, "{id} is on by default");
        }

        let root = dir("own");
        f.accounts = vec![account(&root, Provider::Codex, "one", &[])];
        let list = evaluate(&f);
        let c = check(&list, "codex-hooks-unasked", None);
        assert_eq!(c.status, Status::Off);
        assert!(c.risk.as_deref().unwrap().contains("every project's hooks without review"));
        assert!(apply(&f, "codex-hooks-unasked", None, false).unwrap_err().contains("confirmed"), "not without the confirm");

        f.settings.codex_hooks_unasked = true;
        f.settings.codex_hooks_unasked_in.insert("C:/held".into(), false);
        f.settings.codex_hooks_unasked_in.insert("C:/open".into(), true);
        let list = evaluate(&f);
        let c = check(&list, "codex-hooks-unasked", None);
        assert_eq!(c.status, Status::Ok);
        assert!(c.risk.is_none() && c.action.is_none());
        let note = c.note.as_deref().unwrap();
        assert!(note.contains("C:/held") && !note.contains("C:/open"), "{note}");

        let mut s = Settings {
            waiting_marker_on: false,
            waiting_marker: " ".into(),
            keep_awake: false,
            fetch_before_worktree: false,
            ..Default::default()
        };
        for o in own() {
            (o.fix)(&mut s);
            assert!((o.ok)(&s), "{}", o.id);
        }
        assert_eq!(s.waiting_marker, Settings::default().waiting_marker);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_dismissed_check_stays_listed_for_its_account_only() {
        let root = dir("dismiss");
        let mut f = facts(vec![account(&root, Provider::Claude, "a", &[]), account(&root, Provider::Claude, "b", &[])]);
        f.settings.recommendations_dismissed =
            vec![dismiss_key("claude-bypass-consent", Some("claude/a")), dismiss_key("keep-awake", None)];
        f.settings.keep_awake = false;
        let list = evaluate(&f);
        assert!(check(&list, "claude-bypass-consent", Some("claude/a")).dismissed);
        assert!(!check(&list, "claude-bypass-consent", Some("claude/b")).dismissed);
        let awake = check(&list, "keep-awake", None);
        assert!(awake.dismissed && awake.status == Status::Off);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn turning_claudes_hooks_back_on_says_what_comes_with_it_and_writes_the_parent_too() {
        let root = dir("hooks");
        let off = r#"{"disableAllHooks":true,"model":"opus","hooks":{"Stop":[]}}"#;
        let (name, parent, sibling) = ("kid", "dad", "sib");
        let f = {
            let mut f = facts(vec![
                account(&root, Provider::Claude, name, &[("settings.json", off)]),
                account(&root, Provider::Claude, parent, &[("settings.json", off)]),
                account(&root, Provider::Claude, sibling, &[("settings.json", "{}")]),
            ]);
            f.settings.settings_parents.claude = Some(parent.into());
            f
        };
        let list = evaluate(&f);
        let c = check(&list, "claude-hooks", Some("claude/kid"));
        assert_eq!(c.status, Status::Off);
        let risk = c.risk.as_deref().unwrap();
        assert!(risk.contains("Every hook of this account comes back"), "{risk}");
        assert!(risk.contains("settings parent dad") && risk.contains("copies it to sib"), "{risk}");
        assert!(check(&list, "claude-hooks", Some("claude/dad")).risk.as_deref().unwrap().contains("copies it to kid, sib"));
        assert_eq!(check(&list, "claude-hooks", Some("claude/sib")).status, Status::Ok);

        assert!(apply(&f, "claude-hooks", Some("claude/kid"), false).unwrap_err().contains("confirmed"));
        assert!(read(&f.accounts[0].path, "settings.json").contains("disableAllHooks"), "unconfirmed, unwritten");
        apply(&f, "claude-hooks", Some("claude/kid"), true).unwrap();
        for a in &f.accounts[..2] {
            let v: Value = serde_json::from_str(&read(&a.path, "settings.json")).unwrap();
            assert!(v.get("disableAllHooks").is_none() && v["model"] == "opus" && v["hooks"].is_object());
        }
        assert_eq!(read(&f.accounts[2].path, "settings.json"), "{}", "no key of the sibling's is touched");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_parent_with_nothing_to_fix_is_left_alone_and_one_that_will_not_read_stops_the_fix() {
        let root = dir("parent");
        let mut f = facts(vec![
            account(&root, Provider::Claude, "kid", &[("settings.json", r#"{"disableAllHooks":true}"#)]),
            account(&root, Provider::Claude, "dad", &[("settings.json", "{}")]),
        ]);
        f.settings.settings_parents.claude = Some("dad".into());
        let c = evaluate(&f);
        assert!(!check(&c, "claude-hooks", Some("claude/kid")).risk.as_deref().unwrap().contains("dad"), "the parent plays no part");
        apply(&f, "claude-hooks", Some("claude/kid"), true).unwrap();
        assert_eq!(read(&f.accounts[1].path, "settings.json"), "{}");

        let mut f = facts(vec![
            account(&root, Provider::Claude, "kid", &[("settings.json", r#"{"disableAllHooks":true}"#)]),
            account(&root, Provider::Claude, "dad", &[("settings.json", "{ not json")]),
        ]);
        f.settings.settings_parents.claude = Some("dad".into());
        assert_eq!(check(&evaluate(&f), "claude-hooks", Some("claude/dad")).status, Status::Unknown);
        assert!(apply(&f, "claude-hooks", Some("claude/kid"), true).is_err());
        assert!(read(&f.accounts[0].path, "settings.json").contains("disableAllHooks"));
        assert!(apply(&f, "claude-hooks", Some("claude/dad"), true).is_err(), "unknown is no target");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bypass_consent_is_set_with_the_warning_shown_first() {
        let root = dir("consent");
        let f = facts(vec![account(&root, Provider::Claude, "a", &[("settings.json", r#"{"model":"opus"}"#)])]);
        let list = evaluate(&f);
        let c = check(&list, "claude-bypass-consent", Some("claude/a"));
        assert_eq!((c.status, c.current.as_deref()), (Status::Off, Some("not set")));
        assert!(c.risk.is_some());
        apply(&f, "claude-bypass-consent", Some("claude/a"), true).unwrap();
        let v: Value = serde_json::from_str(&read(&f.accounts[0].path, "settings.json")).unwrap();
        assert_eq!(v, json!({ "model": "opus", "skipDangerousModePermissionPrompt": true }));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn fullscreen_account(root: &Path, version: &str) -> Account {
        let text = format!(
            r#"{{"numStartups":3,"fullscreenAutoDisabled":{{"version":"{version}","at":1}},"fullscreenBootStrikes":{{"count":2,"version":"{version}"}},"fullscreenBootPending":{{"7":{{}}}}}}"#
        );
        account(root, Provider::Claude, "fs", &[(".claude.json", &text)])
    }

    #[test]
    fn fullscreen_counts_for_the_cli_in_use_and_comes_back_with_the_rest_of_the_file_kept() {
        let root = dir("fullscreen");
        let f = facts(vec![fullscreen_account(&root, "2.1.318")]);
        let list = evaluate(&f);
        let c = check(&list, "claude-fullscreen", Some("claude/fs"));
        assert_eq!(c.status, Status::Off);
        assert!(c.risk.is_none(), "the file does not travel and nothing else comes back");
        assert!(c.note.as_deref().unwrap().contains("outside Luna can still race"));
        apply(&f, "claude-fullscreen", Some("claude/fs"), false).unwrap();
        let v: Value = serde_json::from_str(&read(&f.accounts[0].path, ".claude.json")).unwrap();
        assert_eq!(v, json!({ "numStartups": 3, "fullscreenBootPending": { "7": {} } }));

        // An older CLI's mark means nothing to this one.
        let f = facts(vec![fullscreen_account(&root, "2.1.300")]);
        assert_eq!(check(&evaluate(&f), "claude-fullscreen", Some("claude/fs")).status, Status::Ok);
        // The classic renderer by name: not for Luna to argue with.
        let mut a = fullscreen_account(&root, "2.1.318");
        a.main = Ok(Doc::Json(json!({ "tui": "default" })));
        assert_eq!(check(&evaluate(&facts(vec![a])), "claude-fullscreen", Some("claude/fs")).status, Status::Ok);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_cli_on_its_way_out_keeps_the_fix_away_until_its_process_has_exited() {
        let root = dir("retiring");
        let mut a = fullscreen_account(&root, "2.1.318");
        // A chat being deleted: out of the map, its process still running.
        let process = crate::account_use::running_on(&a.path);
        a.in_use = crate::pty::manager().account_in_use(&a.path);
        assert!(a.in_use, "in use until it has exited");
        let before = read(&a.path, ".claude.json");
        let mut f = facts(vec![a]);
        let c = evaluate(&f);
        let row = check(&c, "claude-fullscreen", Some("claude/fs"));
        assert!(row.refused.as_deref().unwrap().contains("in use"), "{row:?}");
        assert!(apply(&f, "claude-fullscreen", Some("claude/fs"), false).unwrap_err().contains("in use"));

        // Read as free a moment before the process counted: the hold is what
        // catches it.
        f.accounts[0].in_use = false;
        assert!(apply(&f, "claude-fullscreen", Some("claude/fs"), false).unwrap_err().contains("in use"));
        assert_eq!(read(&f.accounts[0].path, ".claude.json"), before);

        drop(process);
        f.accounts[0].in_use = crate::pty::manager().account_in_use(&f.accounts[0].path);
        assert!(!f.accounts[0].in_use);
        apply(&f, "claude-fullscreen", Some("claude/fs"), false).unwrap();
        assert!(!read(&f.accounts[0].path, ".claude.json").contains("fullscreenAutoDisabled"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_codex_file_is_written_only_while_every_account_it_touches_is_free() {
        let root = dir("codex");
        let off = "# mine\nmodel = \"x\"\n\n[features]\nhooks = false\n";
        let mut f = facts(vec![
            account(&root, Provider::Codex, "kid", &[("config.toml", off)]),
            account(&root, Provider::Codex, "dad", &[("config.toml", off)]),
            account(&root, Provider::Codex, "sib", &[("config.toml", "")]),
        ]);
        f.settings.settings_parents.codex = Some("dad".into());
        f.settings.codex_hooks_unasked = true;
        let list = evaluate(&f);
        let c = check(&list, "codex-hooks-feature", Some("codex/kid"));
        let risk = c.risk.as_deref().unwrap();
        assert!(risk.contains("Every enabled hook") && risk.contains("with no review"), "{risk}");
        assert!(risk.contains("settings parent dad") && risk.contains("copies it to sib"), "{risk}");
        assert!(c.note.as_deref().unwrap().contains("A Codex running outside Luna can still race"));

        // The parent in use stops it, the account itself being free.
        let on_parent = crate::account_use::running_on(&f.accounts[1].path);
        f.accounts[1].in_use = true;
        let list = evaluate(&f);
        assert!(check(&list, "codex-hooks-feature", Some("codex/kid")).refused.as_deref().unwrap().contains("dad is in use"));
        assert!(apply(&f, "codex-hooks-feature", Some("codex/kid"), true).is_err());
        assert_eq!(read(&f.accounts[0].path, "config.toml"), off);
        drop(on_parent);
        f.accounts[1].in_use = false;

        apply(&f, "codex-hooks-feature", Some("codex/kid"), true).unwrap();
        for a in &f.accounts[..2] {
            assert_eq!(read(&a.path, "config.toml"), "# mine\nmodel = \"x\"\n");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hooks_off_in_an_inline_table_or_under_the_old_name_are_seen_and_fixed() {
        let root = dir("inline");
        for (name, text) in [
            ("inline", "features = { hooks = false }\n"),
            ("alias", "[features]\ncodex_hooks = false\n"),
            ("dotted", "features.hooks = false\n"),
        ] {
            let f = facts(vec![account(&root, Provider::Codex, name, &[("config.toml", text)])]);
            let label = format!("codex/{name}");
            assert_eq!(check(&evaluate(&f), "codex-hooks-feature", Some(&label)).status, Status::Off, "{name}");
            apply(&f, "codex-hooks-feature", Some(&label), true).unwrap();
            assert_eq!(read(&f.accounts[0].path, "config.toml"), "", "{name}");
        }
        let f = facts(vec![account(&root, Provider::Codex, "on", &[("config.toml", "[features]\nhooks = true\n")])]);
        assert_eq!(check(&evaluate(&f), "codex-hooks-feature", Some("codex/on")).status, Status::Ok);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_fix_then_a_sync_reaches_the_sibling_when_it_empties_the_parent() {
        let root = dir("empty-parent");
        let off = "[features]\nhooks = false\n";
        let mut f = facts(vec![
            account(&root, Provider::Codex, "dad", &[("config.toml", off)]),
            account(&root, Provider::Codex, "kid", &[("config.toml", off)]),
            account(
                &root,
                Provider::Codex,
                "sib",
                &[("config.toml", "model = \"mine\"\n[features]\nhooks = false\n[projects.'C:\\\\s']\ntrust_level = \"trusted\"\n")],
            ),
        ]);
        f.settings.settings_parents.codex = Some("dad".into());
        apply(&f, "codex-hooks-feature", Some("codex/kid"), true).unwrap();
        assert_eq!(read(&f.accounts[0].path, "config.toml"), "", "the fix took the parent's only key");
        crate::account_settings::sync_one(Provider::Codex, &f.accounts[0].path, &f.accounts[2].path).unwrap();
        let sib: toml_edit::DocumentMut = read(&f.accounts[2].path, "config.toml").parse().unwrap();
        assert!(sib.get("features").is_none() && sib.get("model").is_none(), "removals travel: {sib}");
        assert!(sib.get("projects").is_some(), "its own folders stay: {sib}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_parent_changed_since_the_row_was_shown_is_seen_when_it_comes_to_writing() {
        let root = dir("fresh-parent");
        let off = r#"{"disableAllHooks":true}"#;
        let mut f = facts(vec![
            account(&root, Provider::Claude, "kid", &[("settings.json", off)]),
            account(&root, Provider::Claude, "dad", &[("settings.json", "{}")]),
        ]);
        f.settings.settings_parents.claude = Some("dad".into());
        assert!(!check(&evaluate(&f), "claude-hooks", Some("claude/kid")).risk.as_deref().unwrap().contains("dad"));
        // The editor, after the page last looked.
        std::fs::write(Path::new(&f.accounts[1].path).join("settings.json"), off).unwrap();
        let e = apply(&f, "claude-hooks", Some("claude/kid"), true).unwrap_err();
        assert!(e.contains("look again"), "{e}");
        assert_eq!(read(&f.accounts[0].path, "settings.json"), off, "half a fix a sync would undo");
        // Looked at afresh, the row says so and the yes covers both.
        f.accounts[1].main = KeyFile::ClaudeSettings.read(&f.accounts[1].path);
        assert!(check(&evaluate(&f), "claude-hooks", Some("claude/kid")).risk.as_deref().unwrap().contains("dad"));
        apply(&f, "claude-hooks", Some("claude/kid"), true).unwrap();
        for a in &f.accounts {
            assert!(!read(&a.path, "settings.json").contains("disableAllHooks"));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_parent_is_held_with_its_child_though_it_looks_healthy() {
        let root = dir("held-parent");
        let mut f = facts(vec![
            account(&root, Provider::Codex, "kid", &[("config.toml", "[features]\nhooks = false\nx = 1\n")]),
            account(&root, Provider::Codex, "dad", &[("config.toml", "x = 1\n")]),
        ]);
        f.settings.settings_parents.codex = Some("dad".into());
        let on_parent = crate::account_use::running_on(&f.accounts[1].path);
        let e = apply(&f, "codex-hooks-feature", Some("codex/kid"), true).unwrap_err();
        assert!(e.contains("dad is in use"), "{e}");
        drop(on_parent);
        apply(&f, "codex-hooks-feature", Some("codex/kid"), true).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_hooks_feature_is_judged_as_codex_ends_up_with_it_not_by_the_account_file_alone() {
        let root = dir("system");
        let system = root.join("config.toml");
        assert_eq!(codex_system_hooks(&system).unwrap(), None);
        std::fs::write(&system, "[features]\nhooks = false\n").unwrap();
        assert_eq!(codex_system_hooks(&system).unwrap(), Some(false));
        std::fs::write(&system, "x = [").unwrap();
        assert!(codex_system_hooks(&system).is_err());

        let row = |f: &Facts, name: &str| check(&evaluate(f), "codex-hooks-feature", Some(&format!("codex/{name}"))).clone();
        // Nothing in the account, and the machine has it off.
        let mut f = facts(vec![account(&root, Provider::Codex, "clean", &[("config.toml", "model = \"x\"\n")])]);
        f.codex_default_hooks = Some(false);
        let c = row(&f, "clean");
        assert_eq!(c.status, Status::Off);
        assert!(c.current.as_deref().unwrap().contains("defaults"), "the file is not what says so: {c:?}");
        assert_eq!(c.target.as_deref(), Some("features.hooks = true"));
        apply(&f, "codex-hooks-feature", Some("codex/clean"), true).unwrap();
        assert_eq!(hooks_said(&KeyFile::CodexConfig.read(&f.accounts[0].path).unwrap()), Some(true));

        // The account's word goes over the machine's, either way.
        let mut f = facts(vec![account(&root, Provider::Codex, "own", &[("config.toml", "[features]\nhooks = true\n")])]);
        f.codex_default_hooks = Some(false);
        assert_eq!(row(&f, "own").status, Status::Ok);
        let mut f = facts(vec![account(&root, Provider::Codex, "mine", &[("config.toml", "[features]\nhooks = false\n")])]);
        f.codex_default_hooks = Some(true);
        let c = row(&f, "mine");
        assert_eq!((c.status, c.target.as_deref()), (Status::Off, Some("key removed")));
        assert!(c.current.as_deref().unwrap().contains("features.hooks = false"));
        apply(&f, "codex-hooks-feature", Some("codex/mine"), true).unwrap();
        assert_eq!(read(&f.accounts[0].path, "config.toml"), "");

        // Off in both: a key taken out alone would leave it off.
        for (name, text) in [
            ("both", "[features]\nhooks = false\n"),
            ("old", "[features]\ncodex_hooks = false\n"),
            ("inline", "features = { hooks = false }\n"),
        ] {
            let mut f = facts(vec![account(&root, Provider::Codex, name, &[("config.toml", text)])]);
            f.codex_default_hooks = Some(false);
            apply(&f, "codex-hooks-feature", Some(&format!("codex/{name}")), true).unwrap();
            assert_eq!(hooks_said(&KeyFile::CodexConfig.read(&f.accounts[0].path).unwrap()), Some(true), "{name}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_that_will_not_read_is_unknown_and_stays_as_it_is() {
        let root = dir("broken");
        let f = facts(vec![
            account(&root, Provider::Claude, "c", &[("settings.json", "{ not json")]),
            account(&root, Provider::Codex, "x", &[("config.toml", "a = [")]),
        ]);
        let list = evaluate(&f);
        for (id, account_label) in [("claude-hooks", "claude/c"), ("claude-bypass-consent", "claude/c"), ("codex-hooks-feature", "codex/x")]
        {
            let c = check(&list, id, Some(account_label));
            assert!(c.status == Status::Unknown && c.action.is_none() && c.note.is_some(), "{c:?}");
            assert!(apply(&f, id, Some(account_label), true).is_err());
        }
        assert_eq!(read(&f.accounts[0].path, "settings.json"), "{ not json");
        assert_eq!(read(&f.accounts[1].path, "config.toml"), "a = [");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_fix_works_from_the_file_as_it_is_and_an_id_is_not_a_key() {
        let root = dir("stale");
        let f = facts(vec![account(&root, Provider::Claude, "a", &[("settings.json", r#"{"disableAllHooks":true}"#)])]);
        // Fixed by hand after the page last looked.
        std::fs::write(Path::new(&f.accounts[0].path).join("settings.json"), r#"{"model":"opus"}"#).unwrap();
        apply(&f, "claude-hooks", Some("claude/a"), true).unwrap();
        assert_eq!(read(&f.accounts[0].path, "settings.json"), r#"{"model":"opus"}"#, "nothing left to do, nothing written");
        assert!(apply(&f, "permissions.defaultMode", Some("claude/a"), true).is_err());
        assert!(apply(&f, "claude-hooks", Some("claude/nobody"), true).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
