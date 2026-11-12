//! The least-privilege layer. A helper runs with no more than its caller may
//! (`settle_mode`), sits in no folder that holds Luna (`reaches_luna`), and text
//! or keys reach a session only as far as the caller's own rights go
//! (`command_reach`).

use crate::agents::ask::Ask;
use crate::agents::registry::{is_descendant, reg, DEV_CALLER};
use crate::agents::spawn::background::{asked, CLAUDE_MODES};
use crate::agents::spawn::SpawnParams;
use crate::provider::Provider;
use std::sync::Mutex;

/// An agent's name as Claude Code takes one, a plugin's `plugin:name` too, and
/// a guard against one being read as a flag.
fn agent_name_ok(name: &str) -> bool {
    !name.starts_with('-') && name.chars().all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c))
}

/// `readOnly` and `agent` checked, for a helper however it starts: what cannot
/// hold together is refused, and a read-only Codex gets its sandbox here.
pub(super) fn settle_role(params: &mut SpawnParams, provider: Provider) -> Result<(), String> {
    use crate::codex::launch::{AUTO_REVIEW, AUTO_REVIEW_SANDBOX, READ_ONLY_NET};
    params.agent = asked(&params.agent).map(str::to_owned);
    if let Some(agent) = params.agent.as_deref() {
        if provider == Provider::Codex {
            return Err("agent is a Claude Code option: Codex cannot start a session as a named agent; its roles \
                        are for the subagents a session spawns itself"
                .into());
        }
        if !agent_name_ok(agent) {
            return Err(format!("\"{agent}\" is not an agent's name"));
        }
    }
    if !params.read_only.unwrap_or(false) {
        return Ok(());
    }
    match provider {
        Provider::Claude => {
            if asked(&params.permission_mode) == Some("acceptEdits") {
                return Err("readOnly takes the edit tools away, so acceptEdits has nothing to accept; leave one out".into());
            }
        }
        Provider::Codex => {
            if asked(&params.approval) == Some(AUTO_REVIEW) {
                return Err(format!("{AUTO_REVIEW} runs in {AUTO_REVIEW_SANDBOX}, which is not read-only; leave one out"));
            }
            match asked(&params.sandbox) {
                None => params.sandbox = Some("read-only".into()),
                Some("read-only") | Some(READ_ONLY_NET) => {}
                Some(s) => {
                    return Err(format!("readOnly contradicts sandbox \"{s}\"; leave sandbox out, or use read-only or {READ_ONLY_NET}"))
                }
            }
        }
    }
    Ok(())
}

/// How far a session acts without a person saying yes, on one scale for both
/// CLIs, so a helper of either provider can be held to its caller's. Each step
/// pairs what the two let through unasked; network is a dimension of its own
/// (`Privilege::net`). Approximations, all leaning to refuse:
/// - `ReadOnly`: Claude Code's `plan`; Codex's `read-only` or `read-only-net`
///   with approval `never`.
/// - `Asks`: `default` and `dontAsk` (which runs what the allow rules let
///   through and refuses the rest, as `default` runs those and asks for the
///   rest; the rules themselves Luna does not weigh); Codex read-only with
///   approval asked of a person.
/// - `Edits`: `acceptEdits`, file edits only.
/// - `Commands`: Codex `workspace-write`: any command, held in the sandbox.
/// - `Reviewed`: Codex `auto-review`, Claude Code's `auto`: a model, not a
///   person, answers what would go further.
/// - `Full`: `bypassPermissions`; Codex `danger-full-access`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) enum Reach {
    ReadOnly,
    Asks,
    Edits,
    Commands,
    Reviewed,
    Full,
}

/// `Reach`, and whether the network is open without asking: Codex's
/// `read-only-net`, `auto-review` and `danger-full-access`, Claude Code's
/// `auto` and `bypassPermissions`. Claude Code asks before a fetch in the
/// others; Codex's `workspace-write` has none unless its config opens it,
/// which Luna does not read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Privilege {
    pub(super) reach: Reach,
    pub(super) net: bool,
}

/// What Luna starts helpers in, spelled exactly as the CLIs take them.
const CODEX_SANDBOXES: [&str; 4] = ["read-only", crate::codex::launch::READ_ONLY_NET, "workspace-write", "danger-full-access"];

/// A helper runs in the background, where nobody is there to ask: it never
/// asks, or a reviewer model answers for it.
const CODEX_APPROVALS: [&str; 2] = ["never", crate::codex::launch::AUTO_REVIEW];

/// Codex's policies that ask a person, which a chat of the person's may run in
/// and a helper may not.
const CODEX_ASKING: [&str; 3] = ["on-request", "untrusted", "on-failure"];

impl Privilege {
    const LEAST: Privilege = Privilege { reach: Reach::ReadOnly, net: false };

    /// None for a mode Luna does not know.
    fn of(provider: Provider, mode: &str, approval: Option<&str>) -> Option<Privilege> {
        use crate::codex::launch::{AUTO_REVIEW, READ_ONLY_NET};
        let at = |reach, net| Some(Privilege { reach, net });
        match provider {
            Provider::Claude => match mode {
                "plan" => at(Reach::ReadOnly, false),
                // `manual` is Claude Code's alias of `default` (2.1.322).
                "default" | "dontAsk" | "manual" => at(Reach::Asks, false),
                "acceptEdits" => at(Reach::Edits, false),
                "auto" => at(Reach::Reviewed, true),
                "bypassPermissions" => at(Reach::Full, true),
                _ => None,
            },
            Provider::Codex => {
                let asks = match approval.unwrap_or("never") {
                    AUTO_REVIEW => return at(Reach::Reviewed, true),
                    "never" => false,
                    // Codex's own policies a user's chat may run in.
                    a if CODEX_ASKING.contains(&a) => true,
                    _ => return None,
                };
                match mode {
                    "danger-full-access" => at(Reach::Full, true),
                    "workspace-write" => at(Reach::Commands, false),
                    "read-only" | READ_ONLY_NET => at(if asks { Reach::Asks } else { Reach::ReadOnly }, mode == READ_ONLY_NET),
                    _ => None,
                }
            }
        }
    }

    /// A running session's, from the settings it was started with.
    fn of_session(provider: Provider, settings: &serde_json::Value) -> Option<Privilege> {
        match provider {
            Provider::Claude => Privilege::of(provider, settings["permissionMode"].as_str()?, None),
            Provider::Codex => Privilege::of(provider, settings["sandbox"].as_str()?, settings["approval"].as_str()),
        }
    }

    /// A caller's: what Luna cannot read counts as the least.
    pub(super) fn of_caller(provider: Provider, settings: &serde_json::Value) -> Privilege {
        Privilege::of_session(provider, settings).unwrap_or(Privilege::LEAST)
    }

    pub(super) fn covers(self, other: Privilege) -> bool {
        other.reach <= self.reach && (self.net || !other.net)
    }

    pub(super) fn label(self) -> String {
        format!("{:?}{}", self.reach, if self.net { " with network" } else { "" })
    }
}

/// A mode as the spawn would spell it, for an error.
fn mode_label(provider: Provider, mode: &str, approval: Option<&str>) -> String {
    match provider {
        Provider::Claude => format!("Claude permissionMode {mode}"),
        Provider::Codex => format!("Codex sandbox {mode} with approval {}", approval.unwrap_or("never")),
    }
}

/// The mode a helper starts in when the spawn leaves it out: what the request
/// asks for over the account's defaults, as `background_spec` works it out.
/// Claude: (permissionMode, None); Codex: (sandbox, Some(approval)). A Codex
/// helper never asks, unless it has a reviewer: nobody is there to ask.
fn effective_mode(params: &SpawnParams, provider: Provider, account_path: &str, folder: &str) -> (String, Option<String>) {
    use crate::codex::launch::{AUTO_REVIEW, AUTO_REVIEW_SANDBOX};
    match provider {
        Provider::Claude => {
            let mode = asked(&params.permission_mode).map(str::to_owned).unwrap_or_else(|| {
                crate::claude::defaults::flags(account_path, folder)["permissionMode"].as_str().unwrap_or("default").to_string()
            });
            (mode, None)
        }
        Provider::Codex => {
            let d = crate::codex::defaults::flags(account_path, folder);
            let approval = match asked(&params.approval) {
                Some(a) => a.to_string(),
                None => "never".to_string(),
            };
            let sandbox = match asked(&params.sandbox) {
                Some(s) => s.to_string(),
                None if approval == AUTO_REVIEW => AUTO_REVIEW_SANDBOX.to_string(),
                None => d["sandbox"].as_str().unwrap_or("read-only").to_string(),
            };
            (sandbox, Some(approval))
        }
    }
}

/// A helper's mode as Luna starts one, spelled exactly: the window reads a
/// setting more loosely (in any case), so what is compared here has to be
/// what runs.
fn canonical_mode(provider: Provider, mode: &str, approval: Option<&str>, defaulted: bool) -> Result<(), String> {
    use crate::codex::launch::{AUTO_REVIEW, AUTO_REVIEW_SANDBOX};
    let whence = if defaulted { " (the account's or project's default; pass one)" } else { "" };
    match provider {
        Provider::Claude if !CLAUDE_MODES.contains(&mode) => {
            Err(format!("unknown permission mode \"{mode}\"{whence}; use {}, spelled so", CLAUDE_MODES.join(", ")))
        }
        Provider::Claude => Ok(()),
        Provider::Codex => {
            let approval = approval.unwrap_or("never");
            if !CODEX_SANDBOXES.contains(&mode) {
                return Err(format!("unknown sandbox \"{mode}\"{whence}; use {}, spelled so", CODEX_SANDBOXES.join(", ")));
            }
            if CODEX_ASKING.contains(&approval) {
                return Err(format!(
                    "approval {approval} asks a person, and a helper runs in the background with nobody there to ask; \
                     use {}",
                    CODEX_APPROVALS.join(" or ")
                ));
            }
            if !CODEX_APPROVALS.contains(&approval) {
                return Err(format!("unknown approval \"{approval}\"{whence}; use {}, spelled so", CODEX_APPROVALS.join(", ")));
            }
            if approval == AUTO_REVIEW && mode != AUTO_REVIEW_SANDBOX {
                return Err(format!("{AUTO_REVIEW} runs in {AUTO_REVIEW_SANDBOX} only; leave sandbox out"));
            }
            Ok(())
        }
    }
}

/// A helper may do no more unasked than the session that spawns it (see
/// `Privilege`), unless the user has let helpers go above their caller
/// (`helpers_above_caller`). A caller whose own mode Luna cannot read counts
/// as the least. `defaulted`: the mode came from the account's or project's
/// defaults, not the request.
fn cap(
    caller: (Provider, &serde_json::Value),
    helper: (Provider, &str, Option<&str>),
    defaulted: bool,
    above_allowed: bool,
) -> Result<(), String> {
    let (provider, mode, approval) = helper;
    let wants = Privilege::of(provider, mode, approval)
        .ok_or_else(|| format!("{} is not a mode Luna knows", mode_label(provider, mode, approval)))?;
    let has = Privilege::of_caller(caller.0, caller.1);
    if above_allowed || has.covers(wants) {
        return Ok(());
    }
    let yours = match caller.0 {
        Provider::Claude => caller.1["permissionMode"].as_str().map(|m| mode_label(Provider::Claude, m, None)),
        Provider::Codex => caller.1["sandbox"].as_str().map(|s| mode_label(Provider::Codex, s, caller.1["approval"].as_str())),
    }
    .unwrap_or_else(|| "a mode Luna cannot read, taken as read-only".into());
    let whence = if defaulted { " (the account's or project's default, as you left it out)" } else { "" };
    let lower = match provider {
        Provider::Claude => "pass a lower permissionMode",
        Provider::Codex => "pass a lower sandbox or approval",
    };
    Err(format!(
        "a helper may not run with more privilege than its caller: you run in {yours} ({}), the helper would run in \
         {}{whence} ({}). {lower}, at most your own. Only the user can let helpers go above their caller, in \
         Luna's settings",
        has.label(),
        mode_label(provider, mode, approval),
        wants.label()
    ))
}

/// The helper modes on the scale, highest first, each as Luna starts one:
/// what `inherited` picks from where the caller's own does not carry over.
/// Claude's `default` stands for its rung (`dontAsk` is on it too): it asks,
/// and its caller answers within its own privilege.
const CLAUDE_LADDER: [&str; 5] = ["bypassPermissions", "auto", "acceptEdits", "default", "plan"];
const CODEX_LADDER: [(&str, &str); 5] = [
    ("danger-full-access", "never"),
    (crate::codex::launch::AUTO_REVIEW_SANDBOX, crate::codex::launch::AUTO_REVIEW),
    ("workspace-write", "never"),
    (crate::codex::launch::READ_ONLY_NET, "never"),
    ("read-only", "never"),
];

/// The mode a helper inherits from its caller when its spawn names none
/// (`HelperMode::Inherit`): the caller's own, where the helper runs on the same
/// CLI and that mode is one a helper starts in; else the highest mode of the
/// helper's CLI the caller's privilege covers (`Privilege::covers`): a Codex
/// chat that asks a person gives `never` with its sandbox, a Claude Code
/// `acceptEdits` chat a Codex `read-only`. Claude: (permissionMode, None);
/// Codex: (sandbox, Some(approval)); and whether it is the caller's own.
pub(super) fn inherited(caller: (Provider, &serde_json::Value), helper: Provider) -> ((String, Option<String>), bool) {
    let (provider, settings) = caller;
    if provider == helper {
        let mode = settings[if helper == Provider::Claude { "permissionMode" } else { "sandbox" }].as_str().unwrap_or("");
        // As `Privilege::of` reads a Codex chat with none: never.
        let approval = (helper == Provider::Codex).then(|| settings["approval"].as_str().unwrap_or("never"));
        if canonical_mode(helper, mode, approval, false).is_ok() {
            return ((mode.to_string(), approval.map(str::to_owned)), true);
        }
    }
    let has = Privilege::of_caller(provider, settings);
    let covered = |mode: &str, approval: Option<&str>| Privilege::of(helper, mode, approval).is_some_and(|p| has.covers(p));
    let picked = match helper {
        Provider::Claude => CLAUDE_LADDER.iter().find(|m| covered(m, None)).map(|m| (m.to_string(), None)),
        Provider::Codex => CODEX_LADDER.iter().find(|(m, a)| covered(m, Some(a))).map(|(m, a)| (m.to_string(), Some(a.to_string()))),
    };
    // The lowest rung is read-only, which every privilege covers.
    let lowest = match helper {
        Provider::Claude => ("plan".to_string(), None),
        Provider::Codex => ("read-only".to_string(), Some("never".to_string())),
    };
    (picked.unwrap_or(lowest), false)
}

/// A spawn's mode settled before anything is made, on every path: named, or
/// inherited from the caller (`inherit`, the person's setting), or else the
/// account's default; checked for its exact spelling, held to the caller's
/// (unless `above_allowed`), then pinned in the request so the helper starts
/// in what was checked, not in what the window reads from the defaults later.
/// A Codex spawn names its mode with either of sandbox and approval.
pub(super) fn settle_mode(
    params: &mut SpawnParams,
    provider: Provider,
    caller: (Provider, &serde_json::Value),
    account_path: &str,
    folder: &str,
    above_allowed: bool,
    inherit: bool,
) -> Result<(), String> {
    let unnamed = match provider {
        Provider::Claude => asked(&params.permission_mode).is_none(),
        Provider::Codex => asked(&params.sandbox).is_none() && asked(&params.approval).is_none(),
    };
    if unnamed && inherit {
        let ((mode, approval), own) = inherited(caller, provider);
        match provider {
            Provider::Claude => params.permission_mode = Some(mode),
            Provider::Codex => (params.sandbox, params.approval) = (Some(mode), approval),
        }
        params.mode_from = Some(if own { "caller" } else { "callerNearest" });
    } else if unnamed {
        params.mode_from = Some("account");
    }
    let (mode, approval) = effective_mode(params, provider, account_path, folder);
    // An inherited mode is pinned by now: only what the defaults filled in counts.
    let defaulted = match provider {
        Provider::Claude => asked(&params.permission_mode).is_none(),
        Provider::Codex => asked(&params.sandbox).is_none() || asked(&params.approval).is_none(),
    };
    canonical_mode(provider, &mode, approval.as_deref(), defaulted)?;
    cap(caller, (provider, &mode, approval.as_deref()), defaulted, above_allowed)?;
    match provider {
        Provider::Claude => params.permission_mode = Some(mode),
        Provider::Codex => (params.sandbox, params.approval) = (Some(mode), approval),
    }
    Ok(())
}

/// What answering a permission yes amounts to: the tool run unasked, at the
/// reach that runs it. Claude Code's edit tools are edits, its reading tools
/// reading, its fetches the network; anything else (Bash, an MCP tool) and
/// every Codex approval, which lets a command out of its sandbox, is full.
pub(super) fn approval_needs(provider: Provider, ask: &Ask) -> Privilege {
    let at = |reach, net| Privilege { reach, net };
    match (provider, ask.tool.as_deref()) {
        (Provider::Claude, Some(t)) if crate::claude::launch::CLAUDE_EDIT_TOOLS.contains(&t) || t == "MultiEdit" => at(Reach::Edits, false),
        (Provider::Claude, Some("Read" | "Glob" | "Grep" | "LS")) => at(Reach::ReadOnly, false),
        (Provider::Claude, Some("WebFetch" | "WebSearch")) => at(Reach::ReadOnly, true),
        _ => at(Reach::Full, true),
    }
}

/// A folder that is Luna's data folder, inside it or above it: a helper there
/// could edit Luna's settings, `helpersAboveCaller` among them, and lift the
/// cap it is held to. The cap limits the mode a helper runs in, not what its
/// folder lets that mode reach. Compared resolved (links followed) as far as
/// the path exists, the rest put back on, as `library::resolved` has it.
pub(super) fn reaches_luna(folder: &str) -> bool {
    let resolve = |p: &std::path::Path| crate::paths::path_key(&crate::library::resolved(p).to_string_lossy());
    within_or_above(&resolve(std::path::Path::new(folder)), &resolve(&crate::paths::data_dir()))
}

/// Two `path_key`s: one is the other or inside it.
fn within_or_above(a: &str, b: &str) -> bool {
    let sep = std::path::MAIN_SEPARATOR;
    // A Unix root's key is empty, and holds everything.
    let under =
        |inner: &str, outer: &str| outer.is_empty() || inner == outer || inner.strip_prefix(outer).is_some_and(|r| r.starts_with(sep));
    under(a, b) || under(b, a)
}

/// The user lets helpers run with more privilege than the session spawning them.
#[tauri::command]
pub fn helpers_above_caller() -> bool {
    crate::settings::get().helpers_above_caller
}

/// Turning it on lifts the privilege cap for every tooled session (the trust
/// contract, threat model §3), so it takes a person: a native dialog, asked by
/// the window process, that a script in the page can open by calling this but
/// cannot answer. Turning it off takes nothing. The answer is the setting as
/// it stands after, so the switch shows a refusal as off.
#[tauri::command]
pub async fn set_helpers_above_caller<R: tauri::Runtime>(window: tauri::WebviewWindow<R>, on: bool) -> Result<bool, String> {
    // Off the main thread: the dialog blocks until it is answered.
    tauri::async_runtime::spawn_blocking(move || set_above(on, || confirm_above(&window))).await.map_err(|e| e.to_string())?
}

const ABOVE_TITLE: &str = "Let helpers run above their caller?";
const ABOVE_TEXT: &str = "Helpers will be able to run with more privilege than the session that spawns them: \
                          a session in a read-only or asking mode could start one with full access, and answer \
                          or type commands into its helpers.\n\nTurn this on only if you mean to.";

/// A message box owned by the window. On Windows `MessageBoxW` itself, which
/// can make Cancel the default, so a stray Enter leaves the cap on; the
/// dialog plugin's task dialog has no such option.
#[cfg(windows)]
fn confirm_above<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDOK, MB_DEFBUTTON2, MB_ICONWARNING, MB_OKCANCEL};
    // SAFETY: the owner is this window's own handle, or none; the strings
    // live across the call.
    unsafe {
        MessageBoxW(
            window.hwnd().ok(),
            &HSTRING::from(ABOVE_TEXT),
            &HSTRING::from(ABOVE_TITLE),
            MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2,
        ) == IDOK
    }
}

#[cfg(not(windows))]
fn confirm_above<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) -> bool {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    window
        .dialog()
        .message(ABOVE_TEXT)
        .title(ABOVE_TITLE)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom("Turn on".into(), "Cancel".into()))
        .parent(window)
        .blocking_show()
}

/// The setting, changed to `on` where that keeps the cap or `confirm` (the
/// person, in the window) agrees to lifting it; as it was otherwise.
fn set_above(on: bool, confirm: impl FnOnce() -> bool) -> Result<bool, String> {
    static GATE: AboveGate = AboveGate::new();
    GATE.set(on, helpers_above_caller, |v| crate::settings::update(|s| s.helpers_above_caller = v), confirm)
}

/// How `helpersAboveCaller` changes. Turning it off is written at once.
/// Turning it on writes nothing where it is on already (a turn-off between
/// that look and a write would otherwise be undone), and otherwise only after
/// a yes, one dialog at a time, and only if no change was written while the
/// dialog was open: a yes applies to the state it was asked about, never to
/// one that came after. `writes` counts this process's writes, under its lock.
struct AboveGate {
    writes: Mutex<u64>,
    asking: Mutex<()>,
}

impl AboveGate {
    const fn new() -> Self {
        Self { writes: Mutex::new(0), asking: Mutex::new(()) }
    }

    fn set(
        &self,
        on: bool,
        read: impl Fn() -> bool,
        write: impl Fn(bool) -> Result<(), String>,
        confirm: impl FnOnce() -> bool,
    ) -> Result<bool, String> {
        let writes = || self.writes.lock().unwrap_or_else(|e| e.into_inner());
        if !on {
            let mut n = writes();
            write(false)?;
            *n += 1;
            return Ok(false);
        }
        // Held across the dialog; a turn-off does not wait for it.
        let _asking = self.asking.lock().unwrap_or_else(|e| e.into_inner());
        let seen = {
            let n = writes();
            if read() {
                return Ok(true);
            }
            *n
        };
        if !confirm() {
            crate::log::info("agents", "helpers above their caller: not confirmed, left off");
            return Ok(read());
        }
        let mut n = writes();
        if *n != seen {
            crate::log::info("agents", "helpers above their caller: changed while asking, the yes not applied");
            return Ok(read());
        }
        write(true)?;
        *n += 1;
        Ok(true)
    }
}

/// Whether `id` is one of the caller's helpers, and the caller's privilege for
/// `answer_reach`: None where the user lets helpers go above their caller, or
/// for the dev caller, which stands in for the user (`register_dev_caller`).
pub(super) fn answerer(caller: &str, id: &str) -> (bool, Option<Privilege>) {
    let (own, me) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        (is_descendant(&r, caller, id), r.known.get(caller).map(|k| (k.provider, k.settings.clone())))
    };
    let approver = (caller != DEV_CALLER && !crate::settings::get().helpers_above_caller)
        .then(|| me.map_or(Privilege::LEAST, |(p, s)| Privilege::of_caller(p, &s)));
    (own, approver)
}

/// The first non-blank character a CLI's composer reads as a command rather
/// than a prompt: `/` (slash commands; Codex's `/permissions` switches its
/// sandbox with no dialog) and `!` (shell mode: the command runs as the user,
/// outside the model and its sandbox), in both CLIs. Claude Code 2.1.318 has
/// no `#` shortcut (its hints: `!`, `/`, `@` for file paths, `?` for help).
pub(crate) fn command_prefix(text: &str) -> Option<char> {
    text.trim_start().chars().next().filter(|c| matches!(c, '/' | '!'))
}

/// Text sent to a session is a prompt for its model. One the CLI would take as
/// a command goes only from a caller at full access into its own helper
/// (`approver` None: the user's setting), or from the dev caller; it is
/// refused, not escaped, as no escape is known to hold in both CLIs. Into a
/// chat's terminal it never goes: the chats there are the person's. A helper is
/// held to it too: `claude -p` runs a project's custom commands and skills,
/// which can pre-approve tools (`allowed-tools`). Codex's `exec` is not known to
/// read a leading `/` as a command; one rule for both costs it nothing a
/// full-access caller cannot still send.
pub(super) fn command_reach(text: &str, dev: bool, own: bool, approver: Option<Privilege>) -> Result<(), String> {
    let Some(c) = command_prefix(text) else { return Ok(()) };
    let full = Privilege { reach: Reach::Full, net: true };
    if dev || (own && approver.is_none_or(|a| a.covers(full))) {
        return Ok(());
    }
    let what = if c == '/' { "a slash command" } else { "shell mode, a command run as the user" };
    Err(format!(
        "text starting with \"{c}\" is {what} to the CLI, not a prompt: only a caller at full access types one, into \
         its own helpers. Start the text with something else"
    ))
}

/// `command_reach` for a caller and a target as they stand now: checked when
/// text is taken and again when it is typed, held text and resumes included.
pub(crate) fn command_reach_now(caller: &str, id: &str, text: &str) -> Result<(), String> {
    if command_prefix(text).is_none() {
        return Ok(());
    }
    let (own, approver) = answerer(caller, id);
    command_reach(text, caller == DEV_CALLER, own, approver)
}

/// Text an agent sends, refused if it holds what a terminal takes as keys:
/// Esc (and so any escape sequence, the end of a bracketed paste among them),
/// other C0 and C1 controls, DEL. Line breaks and tabs are text.
pub(super) fn plain_text(text: &str) -> Result<(), String> {
    match text.chars().find(|&c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) {
        Some(c) => Err(format!("the text holds control character U+{:04X}, which a terminal takes as a key; send plain text", c as u32)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::answer::answer_reach;
    use crate::agents::ask::{Ask, AskQuestion};
    use crate::agents::spawn::SpawnParams;

    fn role(read_only: bool, f: impl FnOnce(&mut SpawnParams)) -> SpawnParams {
        let mut p = SpawnParams { read_only: Some(read_only), ..Default::default() };
        f(&mut p);
        p
    }

    #[test]
    fn lifting_the_cap_takes_a_persons_yes() {
        // The page asks to turn it on and nobody agrees in the dialog: the
        // setting stays off and nothing is written.
        let mut asked = false;
        let answer = set_above(true, || {
            asked = true;
            false
        });
        assert_eq!(answer, Ok(false));
        assert!(asked, "turning it on must ask");
        assert!(!helpers_above_caller());
    }

    #[test]
    fn a_yes_never_lands_on_a_state_it_was_not_asked_about() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
        let gate = AboveGate::new();
        let (stored, written) = (AtomicBool::new(true), AtomicUsize::new(0));
        let read = || stored.load(SeqCst);
        let write = |v: bool| {
            stored.store(v, SeqCst);
            written.fetch_add(1, SeqCst);
            Ok(())
        };

        // A finds it on already: no dialog and no write, so a turn-off by B
        // right after has nothing of A's to undo it.
        assert_eq!(gate.set(true, read, write, || panic!("asked although on")), Ok(true));
        assert_eq!(written.load(SeqCst), 0, "A wrote the relaxation back without a yes");
        assert_eq!(gate.set(false, read, write, || unreachable!()), Ok(false));
        assert!(!read());

        // A asks; while its dialog is open B turns it off; A's yes then
        // finds a change it was not asked about and is not applied.
        let answer = gate.set(true, read, write, || {
            assert_eq!(gate.set(false, read, write, || unreachable!()), Ok(false));
            true
        });
        assert_eq!(answer, Ok(false));
        assert!(!read(), "a stale yes lifted the cap");

        // A no leaves it off; an uninterrupted yes turns it on.
        assert_eq!(gate.set(true, read, write, || false), Ok(false));
        assert!(!read());
        assert_eq!(gate.set(true, read, write, || true), Ok(true));
        assert!(read());
    }

    #[test]
    fn read_only_codex_goes_to_the_read_only_sandbox_unless_told_which() {
        let mut p = role(true, |_| {});
        settle_role(&mut p, Provider::Codex).unwrap();
        assert_eq!(p.sandbox.as_deref(), Some("read-only"), "no network unless asked for");
        let mut p = role(true, |p| p.sandbox = Some("read-only-net".into()));
        settle_role(&mut p, Provider::Codex).unwrap();
        assert_eq!(p.sandbox.as_deref(), Some("read-only-net"));
        let mut p = role(false, |_| {});
        settle_role(&mut p, Provider::Codex).unwrap();
        assert_eq!(p.sandbox, None, "the account's default");
    }

    #[test]
    fn what_contradicts_read_only_is_refused() {
        for sandbox in ["workspace-write", "danger-full-access"] {
            let mut p = role(true, |p| p.sandbox = Some(sandbox.into()));
            assert!(settle_role(&mut p, Provider::Codex).unwrap_err().contains("contradicts"), "{sandbox}");
        }
        let mut p = role(true, |p| p.approval = Some("auto-review".into()));
        assert!(settle_role(&mut p, Provider::Codex).unwrap_err().contains("auto-review"));
        let mut p = role(true, |p| p.permission_mode = Some("acceptEdits".into()));
        assert!(settle_role(&mut p, Provider::Claude).unwrap_err().contains("acceptEdits"));
        // A checker with Bash and nobody to ask is the point of it.
        let mut p = role(true, |p| p.permission_mode = Some("bypassPermissions".into()));
        settle_role(&mut p, Provider::Claude).unwrap();
    }

    #[test]
    fn an_agent_is_for_claude_code_and_a_name_only() {
        let mut p = role(false, |p| p.agent = Some("checker".into()));
        settle_role(&mut p, Provider::Claude).unwrap();
        assert_eq!(p.agent.as_deref(), Some("checker"));
        assert!(settle_role(&mut p, Provider::Codex).unwrap_err().contains("Claude Code option"));
        let mut p = role(false, |p| p.agent = Some("  ".into()));
        settle_role(&mut p, Provider::Codex).unwrap();
        assert_eq!(p.agent, None, "blank is not asked");
        for bad in ["--dangerously-skip-permissions", "a b", "a/b"] {
            let mut p = role(false, |p| p.agent = Some(bad.into()));
            assert!(settle_role(&mut p, Provider::Claude).is_err(), "{bad}");
        }
        let mut p = role(false, |p| p.agent = Some("plugin:review-2.0_x".into()));
        settle_role(&mut p, Provider::Claude).unwrap();
    }

    #[test]
    fn a_helper_is_held_to_its_callers_privilege_across_both_clis() {
        use serde_json::json;
        let claude = |mode: &str| (Provider::Claude, json!({ "permissionMode": mode }));
        let codex = |sandbox: &str, approval: &str| (Provider::Codex, json!({ "sandbox": sandbox, "approval": approval }));
        let callers = [
            ("plan", claude("plan")),
            ("ro", codex("read-only", "never")),
            ("ro-net", codex("read-only-net", "never")),
            ("default", claude("default")),
            ("dontAsk", claude("dontAsk")),
            ("manual", claude("manual")),
            ("ro-ask", codex("read-only", "on-request")),
            ("acceptEdits", claude("acceptEdits")),
            ("ww", codex("workspace-write", "on-request")),
            ("review", codex("workspace-write", "auto-review")),
            ("bypass", claude("bypassPermissions")),
            ("full", codex("danger-full-access", "never")),
        ];
        // Each helper mode, and the callers that may spawn it.
        let helpers: [((Provider, &str, Option<&str>), &[&str]); 9] = [
            (
                (Provider::Claude, "plan", None),
                &["plan", "ro", "ro-net", "default", "dontAsk", "manual", "ro-ask", "acceptEdits", "ww", "review", "bypass", "full"],
            ),
            (
                (Provider::Codex, "read-only", Some("never")),
                &["plan", "ro", "ro-net", "default", "dontAsk", "manual", "ro-ask", "acceptEdits", "ww", "review", "bypass", "full"],
            ),
            // The network is an escalation of its own.
            ((Provider::Codex, "read-only-net", Some("never")), &["ro-net", "review", "bypass", "full"]),
            (
                (Provider::Claude, "default", None),
                &["default", "dontAsk", "manual", "ro-ask", "acceptEdits", "ww", "review", "bypass", "full"],
            ),
            ((Provider::Claude, "acceptEdits", None), &["acceptEdits", "ww", "review", "bypass", "full"]),
            // Commands in a sandbox go further than edits.
            ((Provider::Codex, "workspace-write", Some("never")), &["ww", "review", "bypass", "full"]),
            ((Provider::Codex, "workspace-write", Some("auto-review")), &["review", "bypass", "full"]),
            ((Provider::Claude, "bypassPermissions", None), &["bypass", "full"]),
            ((Provider::Codex, "danger-full-access", Some("on-request")), &["bypass", "full"]),
        ];
        for ((hp, mode, approval), allowed) in helpers {
            for (name, (cp, cs)) in &callers {
                let r = cap((*cp, cs), (hp, mode, approval), false, false);
                assert_eq!(r.is_ok(), allowed.contains(name), "{name} spawning {hp:?} {mode} {approval:?}: {r:?}");
                if let Err(e) = r {
                    assert!(e.contains(&mode_label(hp, mode, approval)), "names the helper's: {e}");
                    let yours = cs["permissionMode"].as_str().or(cs["sandbox"].as_str()).unwrap();
                    assert!(e.contains(yours), "names the caller's: {e}");
                }
                // The user's setting lets every one through.
                assert_eq!(cap((*cp, cs), (hp, mode, approval), false, true), Ok(()));
            }
        }
        // Left to the defaults, the refusal says so and how to go lower.
        let plan = json!({ "permissionMode": "plan" });
        let asks = json!({ "permissionMode": "default" });
        let e = cap((Provider::Claude, &asks), (Provider::Claude, "bypassPermissions", None), true, false).unwrap_err();
        assert!(e.contains("default, as you left it out") && e.contains("lower permissionMode"), "{e}");
        let e = cap((Provider::Claude, &plan), (Provider::Codex, "workspace-write", Some("never")), true, false).unwrap_err();
        assert!(e.contains("lower sandbox or approval"), "{e}");
        // A caller whose mode is not known counts as the least; a mode Luna
        // does not know is refused, whatever the caller.
        let unknown = json!({});
        assert!(cap((Provider::Claude, &unknown), (Provider::Claude, "default", None), false, false).unwrap_err().contains("cannot read"));
        assert_eq!(cap((Provider::Claude, &unknown), (Provider::Claude, "plan", None), false, false), Ok(()));
        let full = json!({ "permissionMode": "bypassPermissions" });
        assert!(cap((Provider::Claude, &full), (Provider::Codex, "nonsense", Some("never")), false, true).is_err());
    }

    #[test]
    fn a_spawns_mode_is_spelled_exactly_checked_and_pinned_on_every_path() {
        use serde_json::json;
        let ro = json!({ "sandbox": "read-only", "approval": "never" });
        let full = json!({ "sandbox": "danger-full-access", "approval": "never" });
        let asked = |f: fn(&mut SpawnParams)| {
            let mut p = SpawnParams::default();
            f(&mut p);
            p
        };
        // What a looser reading would take is refused here, before any
        // comparison, the setting on or not.
        for above in [false, true] {
            let mut p = asked(|p| (p.sandbox, p.approval) = (Some("DANGER-FULL-ACCESS".into()), Some("never".into())));
            let e = settle_mode(&mut p, Provider::Codex, (Provider::Codex, &ro), "", "", above, false).unwrap_err();
            assert!(e.contains("unknown sandbox \"DANGER-FULL-ACCESS\""), "{e}");
            let mut p = asked(|p| p.permission_mode = Some("BypassPermissions".into()));
            assert!(settle_mode(&mut p, Provider::Claude, (Provider::Codex, &full), "", "", above, false)
                .unwrap_err()
                .contains("unknown permission mode"));
            let mut p = asked(|p| (p.sandbox, p.approval) = (Some("read-only".into()), Some("Never".into())));
            assert!(settle_mode(&mut p, Provider::Codex, (Provider::Codex, &full), "", "", above, false)
                .unwrap_err()
                .contains("unknown approval"));
        }
        // Spelled right, it is held to the caller's and pinned as checked.
        let mut p = asked(|p| (p.sandbox, p.approval) = (Some("danger-full-access".into()), Some("never".into())));
        assert!(settle_mode(&mut p, Provider::Codex, (Provider::Codex, &ro), "", "", false, false).unwrap_err().contains("more privilege"));
        let mut p = asked(|p| (p.sandbox, p.approval) = (Some(" read-only ".into()), Some("never".into())));
        settle_mode(&mut p, Provider::Codex, (Provider::Codex, &ro), "", "", false, false).unwrap();
        assert_eq!((p.sandbox.as_deref(), p.approval.as_deref()), (Some("read-only"), Some("never")));
        // A background Codex never asks unless a reviewer answers; its sandbox is then fixed.
        let mut p = SpawnParams { approval: Some("auto-review".into()), ..Default::default() };
        settle_mode(&mut p, Provider::Codex, (Provider::Codex, &full), "", "", false, false).unwrap();
        assert_eq!((p.sandbox.as_deref(), p.approval.as_deref()), (Some("workspace-write"), Some("auto-review")));
        let mut p = SpawnParams { sandbox: Some("read-only".into()), ..Default::default() };
        settle_mode(&mut p, Provider::Codex, (Provider::Codex, &ro), "", "", false, false).unwrap();
        assert_eq!(p.approval.as_deref(), Some("never"));
    }

    /// A spawn that names no mode takes its caller's, on the same CLI exactly
    /// and across them the highest the caller's privilege covers; never more
    /// than the caller, and what is named still wins.
    #[test]
    fn a_helper_inherits_its_callers_mode_or_the_nearest_below_it() {
        use serde_json::json;
        let claude = |mode: &str| (Provider::Claude, json!({ "permissionMode": mode }));
        let codex = |sandbox: &str, approval: &str| (Provider::Codex, json!({ "sandbox": sandbox, "approval": approval }));
        let s = |m: &str, a: Option<&str>| (m.to_string(), a.map(str::to_owned));
        // The same CLI: the caller's own, auto and dontAsk among them.
        for m in ["plan", "default", "dontAsk", "acceptEdits", "auto", "bypassPermissions"] {
            let (c, settings) = claude(m);
            assert_eq!(inherited((c, &settings), Provider::Claude), (s(m, None), true), "{m}");
        }
        let (c, settings) = codex("workspace-write", "auto-review");
        assert_eq!(inherited((c, &settings), Provider::Codex), (s("workspace-write", Some("auto-review")), true));
        // A Codex chat that asks a person: its sandbox, and never, as nobody is there to ask.
        for (sandbox, want) in
            [("workspace-write", "workspace-write"), ("read-only", "read-only"), ("danger-full-access", "danger-full-access")]
        {
            let (c, settings) = codex(sandbox, "on-request");
            assert_eq!(inherited((c, &settings), Provider::Codex).0, s(want, Some("never")), "{sandbox}");
        }
        // Across CLIs, the highest rung covered, the network apart.
        let across = [
            (claude("plan"), s("read-only", Some("never"))),
            (claude("default"), s("read-only", Some("never"))),
            (claude("acceptEdits"), s("read-only", Some("never"))),
            (claude("auto"), s("workspace-write", Some("auto-review"))),
            (claude("bypassPermissions"), s("danger-full-access", Some("never"))),
            (codex("read-only-net", "never"), s("plan", None)),
            (codex("workspace-write", "never"), s("acceptEdits", None)),
            (codex("workspace-write", "auto-review"), s("auto", None)),
            (codex("danger-full-access", "never"), s("bypassPermissions", None)),
        ];
        for ((c, settings), want) in across {
            let helper = if c == Provider::Claude { Provider::Codex } else { Provider::Claude };
            let ((m, a), own) = inherited((c, &settings), helper);
            assert_eq!(((m.clone(), a.clone()), own), (want.clone(), false), "{settings}");
            let wants = Privilege::of(helper, &m, a.as_deref()).unwrap();
            assert!(Privilege::of_caller(c, &settings).covers(wants), "{settings}: never above the caller");
        }
        // `manual`, Claude Code's own name for `default`, asks as `default` does.
        let (c, manual) = claude("manual");
        assert_eq!(Privilege::of_caller(c, &manual), Privilege { reach: Reach::Asks, net: false });
        assert_eq!(inherited((c, &manual), Provider::Claude), (s("default", None), false));
        assert_eq!(inherited((c, &manual), Provider::Codex).0, s("read-only", Some("never")));
        // A caller whose mode Luna cannot read passes on the least.
        assert_eq!(inherited((Provider::Claude, &json!({})), Provider::Claude), (s("plan", None), false));
        assert_eq!(inherited((Provider::Claude, &json!({})), Provider::Codex).0, s("read-only", Some("never")));

        // Through settle_mode: pinned, said, and held to the caller as any other.
        let (c, auto) = claude("auto");
        let mut p = SpawnParams::default();
        settle_mode(&mut p, Provider::Claude, (c, &auto), "", "", false, true).unwrap();
        assert_eq!((p.permission_mode.as_deref(), p.mode_from), (Some("auto"), Some("caller")));
        let mut p = SpawnParams::default();
        settle_mode(&mut p, Provider::Codex, (c, &auto), "", "", false, true).unwrap();
        assert_eq!(
            (p.sandbox.as_deref(), p.approval.as_deref(), p.mode_from),
            (Some("workspace-write"), Some("auto-review"), Some("callerNearest"))
        );
        // Named, it wins, and is capped as ever.
        let mut p = SpawnParams { permission_mode: Some("plan".into()), ..Default::default() };
        settle_mode(&mut p, Provider::Claude, (c, &auto), "", "", false, true).unwrap();
        assert_eq!((p.permission_mode.as_deref(), p.mode_from), (Some("plan"), None));
        let mut p = SpawnParams { permission_mode: Some("bypassPermissions".into()), ..Default::default() };
        assert!(settle_mode(&mut p, Provider::Claude, (c, &auto), "", "", false, true).unwrap_err().contains("more privilege"));
        // Not inheriting, an unnamed mode is the account's: said so.
        let mut p = SpawnParams::default();
        let _ = settle_mode(&mut p, Provider::Claude, (c, &auto), "C:\\no\\such\\account", "", false, false);
        assert_eq!(p.mode_from, Some("account"));
    }

    /// Helpers run in the background, so a Codex policy that asks a person has
    /// nobody to ask: refused by name, whatever the caller may run and the
    /// user's setting; a chat of the person's still runs in it and counts as
    /// asking when it spawns.
    #[test]
    fn a_codex_helper_never_asks_a_person() {
        use serde_json::json;
        let full = json!({ "sandbox": "danger-full-access", "approval": "never" });
        for approval in ["on-request", "untrusted", "on-failure"] {
            for above in [false, true] {
                let mut p = SpawnParams { approval: Some(approval.into()), sandbox: Some("read-only".into()), ..Default::default() };
                let e = settle_mode(&mut p, Provider::Codex, (Provider::Codex, &full), "", "", above, false).unwrap_err();
                assert!(e.contains("nobody there to ask") && e.contains("never or auto-review"), "{approval}: {e}");
            }
            let chat = json!({ "sandbox": "read-only", "approval": approval });
            assert_eq!(Privilege::of_caller(Provider::Codex, &chat), Privilege { reach: Reach::Asks, net: false });
        }
    }

    #[test]
    fn a_caller_approves_its_helpers_permission_only_where_it_would_run_it_unasked() {
        let permission = |tool: &str| Ask::permission(Some(tool.into()), None);
        let at = |reach, net| Some(Privilege { reach, net });
        let (bash, edit, fetch) = (permission("Bash"), permission("Edit"), permission("WebFetch"));
        // Read-only and asking callers never say yes: it goes to a person.
        for has in [at(Reach::ReadOnly, false), at(Reach::Asks, false), at(Reach::ReadOnly, true)] {
            for p in [&bash, &edit, &fetch] {
                assert!(answer_reach(true, has, Provider::Claude, Some(p), Some(true)).unwrap_err().contains("goes to the user"));
                assert_eq!(answer_reach(true, has, Provider::Claude, Some(p), Some(false)), Ok(()), "a no is always theirs");
            }
        }
        // acceptEdits may pass an edit, not a command or a fetch.
        let edits = at(Reach::Edits, false);
        assert_eq!(answer_reach(true, edits, Provider::Claude, Some(&edit), Some(true)), Ok(()));
        assert!(answer_reach(true, edits, Provider::Claude, Some(&bash), Some(true)).is_err());
        assert!(answer_reach(true, edits, Provider::Claude, Some(&fetch), Some(true)).is_err());
        // A Codex approval lets a command out of its sandbox: full only.
        let codex = Ask::permission(None, Some("rm -rf build".into()));
        assert!(answer_reach(true, at(Reach::Commands, false), Provider::Codex, Some(&codex), Some(true)).is_err());
        assert!(answer_reach(true, at(Reach::Reviewed, true), Provider::Codex, Some(&codex), Some(true)).is_err());
        let full = at(Reach::Full, true);
        for (provider, p) in [(Provider::Codex, &codex), (Provider::Claude, &bash), (Provider::Claude, &fetch)] {
            assert_eq!(answer_reach(true, full, provider, Some(p), Some(true)), Ok(()));
        }
        // The user's setting lifts it (no approver to hold to).
        assert_eq!(answer_reach(true, None, Provider::Claude, Some(&bash), Some(true)), Ok(()));
    }

    #[test]
    fn no_helper_works_in_lunas_own_folder_or_above_it() {
        let sep = std::path::MAIN_SEPARATOR;
        let k = |p: &str| crate::paths::path_key(&p.replace('/', &sep.to_string()));
        let data = k("/apps/luna");
        assert!(within_or_above(&data, &data));
        assert!(within_or_above(&k("/apps/luna/accounts/x"), &data), "inside");
        assert!(within_or_above(&k("/apps"), &data), "above: it holds Luna's folder");
        assert!(within_or_above(&k("/"), &data), "the root holds everything");
        assert!(!within_or_above(&k("/apps/luna-test"), &data), "a sibling whose name starts the same");
        assert!(!within_or_above(&k("/projects/x"), &data));
        assert!(reaches_luna(&crate::paths::data_dir().join("sub").to_string_lossy()));
        assert!(!reaches_luna(&std::env::temp_dir().join("luna-some-project").to_string_lossy()));
    }

    #[test]
    fn sent_text_holds_no_keys() {
        assert_eq!(plain_text("two lines\r\nand\ta tab, \u{fc}nicode, \u{65e5}\u{672c}"), Ok(()));
        for bad in ["\x1b[201~y\r", "\x1b", "a\x03", "\x7f", "\u{9b}2~", "\x00"] {
            assert!(plain_text(bad).unwrap_err().contains("control character"), "{bad:?}");
        }
    }

    #[test]
    fn a_clis_own_lists_take_full_access_even_on_ones_own_helper() {
        let at = |reach, net| Some(Privilege { reach, net });
        let q = |text: &str, list_at| AskQuestion {
            text: text.into(),
            options: vec!["Trust all and continue".into(), "Quit".into()],
            multi: false,
            list_at,
        };
        let question = Ask::question(vec![q("Which?", None)], true);
        let hooks = Ask::question(vec![q("Hooks need review", Some(0))], true);
        for has in [
            at(Reach::ReadOnly, false),
            at(Reach::Asks, false),
            at(Reach::Edits, false),
            at(Reach::Commands, false),
            at(Reach::Reviewed, true),
        ] {
            assert!(answer_reach(true, has, Provider::Codex, Some(&hooks), None).unwrap_err().contains("Hooks need review"));
            assert!(answer_reach(true, has, Provider::Codex, Some(&hooks), Some(true)).is_err());
            assert_eq!(answer_reach(true, has, Provider::Claude, Some(&question), None), Ok(()), "a model's question stays open");
        }
        let full = at(Reach::Full, true);
        assert_eq!(answer_reach(true, full, Provider::Codex, Some(&hooks), None), Ok(()));
        assert_eq!(answer_reach(true, None, Provider::Codex, Some(&hooks), None), Ok(()), "the user's setting");
    }

    #[test]
    fn text_a_cli_reads_as_a_command_goes_only_from_full_access_into_ones_own_helper() {
        for text in ["/permissions :workspace", "! rm -rf build", "!cmd", "  /approve-for-me", "\n\t! whoami", "\u{3000}/x"] {
            assert!(command_prefix(text).is_some(), "{text:?}");
        }
        for text in ["fix /src/main.rs", "why ! there", "# heading", "@file.rs look", "?", "", "   "] {
            assert!(command_prefix(text).is_none(), "{text:?}");
        }
        let at = |reach, net| Some(Privilege { reach, net });
        let full = at(Reach::Full, true);
        for text in ["/permissions :workspace", "  ! cmd"] {
            for has in [at(Reach::ReadOnly, false), at(Reach::Asks, false), at(Reach::Commands, false), at(Reach::Reviewed, true)] {
                for own in [true, false] {
                    assert!(command_reach(text, false, own, has).unwrap_err().contains("not a prompt"), "{text:?} {has:?} own {own}");
                }
            }
            assert_eq!(command_reach(text, false, true, full), Ok(()), "full access, its own helper");
            assert!(command_reach(text, false, false, full).is_err(), "never into another's session, the user's chats included");
            assert_eq!(command_reach(text, false, true, None), Ok(()), "the user's setting, on its own helper");
            assert!(command_reach(text, false, false, None).is_err(), "the setting does not reach others' sessions");
            assert_eq!(command_reach(text, true, false, at(Reach::ReadOnly, false)), Ok(()), "the dev caller");
        }
        assert_eq!(command_reach("plain words", false, false, at(Reach::ReadOnly, false)), Ok(()));
    }
}
