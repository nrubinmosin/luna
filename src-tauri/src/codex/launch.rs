//! How a Codex session in a pane is started, and what a helper's `codex
//! app-server` shares with it (background/): arguments and environment, laid out from
//! what the start has already settled. Pure: nothing here reads the
//! environment, the disk or Luna's state, logs, writes, registers or mints.
//! The start does that under the chat's spawn gate (pty/start.rs) and hands
//! the results in; a helper's turn, in background/command.rs.

use crate::applied::{Applied, Item};
use crate::codex::defaults::OWN;
use crate::pty::MCP_TOOL_TIMEOUT_MS;
use crate::settings::CopyOnSelect;

/// A Codex chat's start as `chat_args` lays it out: what the window asked
/// for, and what the start settled for it under the spawn gate — the trust
/// override, the sandbox's profile, PATH and git entries, Luna's
/// server and the token — which this only passes on.
pub(crate) struct ChatStart<'a> {
    pub(crate) id: &'a str,
    pub(crate) folder: &'a str,
    pub(crate) account_path: &'a str,
    pub(crate) resume: Option<&'a str>,
    /// A `-c` trusting the folder for this run, and what it comes of: a
    /// trusted root, or a trust asked for while the account was in use.
    pub(crate) trust: Option<([String; 2], Item)>,
    /// The account's config names a `windows.sandbox` of its own
    /// (codex/config.rs `windows_sandbox_set`): Luna gives it none.
    pub(crate) own_windows_sandbox: bool,
    pub(crate) worktree: bool,
    pub(crate) model: Option<&'a str>,
    pub(crate) effort: &'a str,
    /// As Luna runs it (codex/defaults.rs `launch_mode`); `OWN`, Codex's own
    /// from its config, gets no flag.
    pub(crate) approval: &'a str,
    /// The sandbox it runs in: `AUTO_REVIEW_SANDBOX` under the reviewer;
    /// `OWN` gets no flag.
    pub(crate) sandbox: &'a str,
    /// The name of Luna's profile for that sandbox, new on every start
    /// (`profile_args`); None where Codex's own mode serves (`takes_profile`).
    pub(crate) profile: Option<&'a str>,
    /// Luna's `core/`, for the profile to keep the session from reading
    /// (`denies_core`), or None.
    pub(crate) deny: Option<&'a str>,
    /// PATH for the sandbox (pty/start.rs `codex_sandbox_path`), where it
    /// takes one.
    pub(crate) sandbox_path: Option<&'a str>,
    /// What the sandbox needs of git's config (pty/start.rs `codex_git_envs`).
    pub(crate) git_envs: &'a [(&'static str, String)],
    /// Luna's MCP server's URL and the session's token, while the server is
    /// up to take it.
    pub(crate) mcp: Option<(&'a str, &'a str)>,
    /// The URL its status hooks post to (hub.rs `hook_url`), with its secret,
    /// while the listener is up, and the curl they post with, an absolute path
    /// (pty/start.rs `codex_hook_curl`): Luna's hooks go only with both.
    pub(crate) hooks: Option<(&'a str, &'a str)>,
    pub(crate) skill_args: &'a [String],
    /// Settings › Chats › Copy on select (`copy_on_select_override`).
    pub(crate) copy_on_select: CopyOnSelect,
}

/// A thread id that may follow `resume` on a command line: the exact thread,
/// never a flag, a name, `--last` or anything the CLI would pick by itself
/// (0.169.0 tries a UUID first, then a thread's name, so `release-review` would
/// resume whichever thread is called that). Codex ids are UUIDs: any form of
/// one is taken and given back as the plain lowercase one. Resuming is bound
/// to the account too: Luna's `CODEX_HOME`
/// is the session's own, and `resume <id>` there fails with "no rollout found
/// for thread id" for a thread that account does not hold, starting no other
/// (0.169.0, seen on a throwaway account), so it is not looked for in another.
pub(crate) fn thread_to_resume(id: &str) -> Result<String, String> {
    let bare = id.trim();
    // The forms `Uuid::parse_str` takes, which Codex resolves a resume argument
    // with before it tries it as a thread's name: plain, simple (no dashes),
    // braced and URN.
    let bare = bare.get(..9).filter(|p| p.eq_ignore_ascii_case("urn:uuid:")).map_or(bare, |_| &bare[9..]);
    let bare = bare.strip_prefix('{').and_then(|b| b.strip_suffix('}')).unwrap_or(bare);
    let hex: String = bare.chars().filter(|c| *c != '-').collect();
    let dashes_ok =
        bare.len() == 32 && !bare.contains('-') || bare.len() == 36 && [8, 13, 18, 23].iter().all(|&i| bare.as_bytes()[i] == b'-');
    if hex.len() != 32 || !dashes_ok || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{id:?} is not a Codex thread id (a UUID): Luna resumes a thread by its id only, never by a name or `--last`"));
    }
    let hex = hex.to_ascii_lowercase();
    Ok(format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]))
}

/// The arguments and environment of a Codex chat's start, and what Luna
/// applied in them (applied.rs); `codex login` takes none of them.
pub(crate) fn chat_args(s: &ChatStart) -> (Vec<String>, Vec<(&'static str, String)>, Vec<Applied>) {
    let mut args: Vec<String> = vec![];
    let mut envs = vec![("CODEX_HOME", s.account_path.to_string())];
    let mut applied = vec![
        Item::Account.with(format!("CODEX_HOME={}", s.account_path)),
        crate::claude::launch::session_applied(s.id, None),
        Item::Folder.with(format!("-C {}", s.folder)),
    ];
    if let Some(session_id) = s.resume {
        args.push("resume".into());
        args.push(session_id.to_string());
        applied.push(Item::Resume.with(format!("resume {session_id}")));
    }
    args.push("-C".into());
    args.push(s.folder.to_string());
    // The session runs in this process, in Luna's pty, never in Codex's
    // shared background server: Luna stops, deletes and counts it as a
    // process of its own. The `-c` overrides below need that anyway;
    // saying so keeps Codex from warning about it on every start.
    args.push("--no-daemon".into());
    applied.push(Item::NoDaemon.with("--no-daemon"));
    if let Some((trust, item)) = &s.trust {
        args.extend(trust.iter().cloned());
        applied.push(item.with(format!("-c {}", trust[1])));
    }
    // A resumed thread already has its worktree; Codex refuses the
    // flag on `resume`.
    if s.worktree && s.resume.is_none() {
        args.push("--worktree".into());
        applied.push(Item::Worktree.with("--worktree"));
    }
    if let Some(m) = s.model {
        args.push("--model".into());
        args.push(m.to_string());
        applied.push(Item::Model.with(format!("--model {m}")));
    }
    args.push("-c".into());
    args.push(format!("model_reasoning_effort=\"{}\"", s.effort));
    applied.push(Item::Effort.with(format!("-c model_reasoning_effort=\"{}\"", s.effort)));
    let (approval, sandbox) = (s.approval, s.sandbox);
    // The pair Codex itself spells as one flag.
    if approval == "never" && sandbox == "danger-full-access" {
        args.push("--dangerously-bypass-approvals-and-sandbox".into());
        applied.push(Item::Approval.with("--dangerously-bypass-approvals-and-sandbox"));
        applied.push(Item::Sandbox.with("danger-full-access, in --dangerously-bypass-approvals-and-sandbox"));
    } else {
        if approval == AUTO_REVIEW {
            args.push("--approve-for-me".into());
            applied.push(Item::Approval.with("--approve-for-me"));
            applied.push(Item::Sandbox.with(format!("{sandbox}, Codex's own under --approve-for-me")));
        } else {
            // `--ask-for-approval` takes `on-request` and `never` alone
            // (0.169.0); Codex's own (`OWN`) gets no flag.
            if approval != OWN {
                args.push("--ask-for-approval".into());
                args.push(approval.to_string());
                applied.push(Item::Approval.with(format!("--ask-for-approval {approval}")));
            }
            // A person's chat keeps whatever reviewer its account or project
            // names (a helper's turn is kept to a person's: background/command.rs).
            // Without its profile, `--sandbox read-only-net`, which Codex
            // refuses: never a sandbox looser than the one asked for.
            match s.profile.filter(|_| takes_profile(sandbox, cfg!(windows))) {
                Some(profile) => {
                    let held = profile_args(profile, sandbox == READ_ONLY_NET, s.deny);
                    applied.push(sandbox_applied(sandbox, &held));
                    args.extend(held);
                    if let Some(dir) = s.deny {
                        applied.push(core_deny_applied(profile, dir));
                    }
                }
                None if sandbox == OWN => {}
                None => {
                    args.push("--sandbox".into());
                    args.push(sandbox.to_string());
                    applied.push(Item::Sandbox.with(format!("--sandbox {sandbox}")));
                }
            }
        }
        if let Some(sb) = codex_windows_sandbox(sandbox).filter(|_| !s.own_windows_sandbox) {
            args.push("-c".into());
            args.push(sb.clone());
            applied.push(Item::WindowsSandbox.with(format!("-c {sb}")));
        }
        if let Some(path) = s.sandbox_path {
            envs.push(("PATH", path.to_string()));
            applied.push(Item::SandboxPath.with("PATH without …\\Microsoft\\WindowsApps"));
        }
        envs.extend(s.git_envs.iter().cloned());
        if !s.git_envs.is_empty() {
            applied.push(git_ssl_applied(s.git_envs));
        }
    }
    args.extend(no_update_check());
    applied.push(Item::NoAutoUpdate.with(format!("-c {NO_UPDATE_CHECK}")));
    // What its TUI does on its own, said at every start and never written
    // into the account (a helper's `exec` has no TUI).
    args.extend(["-c".to_string(), TUI_NOTIFICATIONS.to_string()]);
    applied.push(Item::TuiSettings.with(format!("-c {TUI_NOTIFICATIONS}")));
    if let Some(copy) = copy_on_select_override(s.copy_on_select) {
        args.extend(["-c".to_string(), copy.to_string()]);
        applied.push(Item::CopyOnSelect.with(format!("-c {copy}")));
    }
    args.push("-c".into());
    args.push(CODEX_TITLE.into());
    applied.push(Item::ThreadTitle.with(format!("-c {CODEX_TITLE}")));
    envs.extend(crate::pty::session_envs(s.id, None));
    // Its status hooks, trusted for this run alone (codex/hooks.rs); the URL
    // in the environment, out of the command line.
    if let Some((url, curl)) = s.hooks {
        args.extend(crate::codex::hooks::status_hook_args(curl, cfg!(windows)));
        envs.push((crate::codex::hooks::HOOK_URL_ENV, url.to_string()));
        applied.push(Item::ActivityHooks.with(HOOKS_VALUE));
    }
    if let Some((url, token)) = s.mcp {
        args.extend(mcp_args(url));
        envs.push(("LUNA_MCP_TOKEN", token.to_string()));
        applied.push(Item::LunaTools.with(LUNA_TOOLS_VALUE));
    }
    args.extend(s.skill_args.iter().cloned());
    applied.extend(library_applied(s.skill_args));
    (args, envs, applied)
}

/// Luna's MCP server for Codex as applied: no URL, the token by the variable
/// it travels in.
pub(crate) const LUNA_TOOLS_VALUE: &str = "-c mcp_servers.luna.url, mcp_servers.luna.bearer_token_env_var, \
     mcp_servers.luna.tool_timeout_sec, mcp_servers.luna.default_tools_approval_mode (Luna's MCP server, its tools \
     approved); its token in LUNA_MCP_TOKEN, not shown";

/// Luna's status hooks for Codex as applied: the overrides by key, the URL by
/// the variable it travels in.
pub(crate) const HOOKS_VALUE: &str = "-c hooks.UserPromptSubmit, -c hooks.PostToolUse, -c hooks.PermissionRequest, \
     -c hooks.Stop, -c hooks.SubagentStart, -c hooks.SubagentStop, -c hooks.Interrupt, -c hooks.SessionEnd (Luna's status \
     hooks), -c hooks.state (each trusted by its hash, for this run alone); their URL in LUNA_HOOK_URL, not shown";

/// A sandbox held by the `-c` overrides `held` (`profile_args`,
/// `sandbox_args`), as applied: the overrides themselves, none secret.
pub(crate) fn sandbox_applied(sandbox: &str, held: &[String]) -> Applied {
    Item::Sandbox.with(format!("{sandbox}: {}", held.join(" ")))
}

/// Luna's profile keeping `dir`, its `core/`, from the session, as applied.
pub(crate) fn core_deny_applied(profile: &str, dir: &str) -> Applied {
    Item::CoreReadDeny.with(format!("-c permissions.{profile}.filesystem={{{dir}=\"deny\"}}"))
}

/// git's TLS backend for the sandbox (`git_ssl_backend_envs`), as applied.
pub(crate) fn git_ssl_applied(envs: &[(&'static str, String)]) -> Applied {
    Item::GitSslBackend.with(envs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", "))
}

/// The library's `-c`s for Codex (library/folders.rs `session_args`): its
/// folders' plugins turned on, and the skills they keep from the session.
pub(crate) fn library_applied(skill_args: &[String]) -> Vec<Applied> {
    let values = skill_args.iter().filter(|a| *a != "-c");
    let (hidden, plugins): (Vec<&String>, Vec<&String>) = values.partition(|v| v.starts_with("skills.config="));
    let mut applied = Vec::new();
    if !plugins.is_empty() {
        applied.push(Item::LibrarySkills.with(plugins.iter().map(|p| format!("-c {p}")).collect::<Vec<_>>().join(", ")));
    }
    if !hidden.is_empty() {
        applied.push(Item::LibraryHidden.with(hidden.iter().map(|p| format!("-c {p}")).collect::<Vec<_>>().join(", ")));
    }
    applied
}

/// Codex's own check for a newer version at its start: Luna updates the CLI
/// itself, and the check would only announce a version Luna is about to
/// install. Passed with every start, never written to the account's
/// config.toml.
pub(crate) const NO_UPDATE_CHECK: &str = "check_for_update_on_startup=false";

/// `NO_UPDATE_CHECK` as a `-c` override.
pub(crate) fn no_update_check() -> [String; 2] {
    ["-c".into(), NO_UPDATE_CHECK.into()]
}

/// What a Codex chat's TUI is told at every start, as a `-c` override (seen
/// in 0.169.0, docs/cli-compatibility.md): no desktop notification, as Luna
/// says when a chat is waiting.
pub(crate) const TUI_NOTIFICATIONS: &str = "tui.notifications=false";

/// Settings › Chats › Copy on select as a Codex chat's `-c` override, never
/// written into the account: `never` for off, so the clipboard keeps what the
/// person put there until they copy, `always` for on, and none where it is
/// left to the account's config. 0.171.0 takes `auto`, `always` or `never`,
/// strings only (docs/cli-compatibility.md).
pub(crate) fn copy_on_select_override(copy: CopyOnSelect) -> Option<&'static str> {
    match copy {
        CopyOnSelect::Off => Some("tui.copy_on_select=\"never\""),
        CopyOnSelect::On => Some("tui.copy_on_select=\"always\""),
        CopyOnSelect::Account => None,
    }
}

/// What `--approve-for-me` stands for, as `-c` overrides: Codex 0.167.0 turns the
/// flag into exactly these (`take_auto_review_config_overrides`,
/// utils/cli/src/shared_options.rs at rust-v0.167.0). `codex app-server` has
/// no such flag but takes `-c`, so a helper's turns pass these.
pub(crate) const AUTO_REVIEW_OVERRIDES: [&str; 3] =
    ["approvals_reviewer=\"auto_review\"", "approval_policy=\"on-request\"", "sandbox_mode=\"workspace-write\""];

/// What keeps a Codex helper to Luna's MCP server, as a Claude Code one is kept
/// by `--strict-mcp-config`, for its run alone: its account's apps (ChatGPT's
/// connectors, the stable `features.apps`); each server the layers of its run
/// define, the account's, the system's and the project's (`mcp.servers`,
/// codex/config.rs `helper_mcp`), with `enabled = false`; each plugin on in
/// them but the library's (`mcp.plugins`) off, its servers with it, and those
/// no layer names (installed from ChatGPT) by `plugins._default`. Off a
/// plugin's skills go too: its servers can be turned off only by names in its
/// own manifest, which Luna does not read.
///
/// Each list is one `-c` whose value is a table, as Codex 0.169.0 merges it
/// into the layers below key by key: a name with a dot in it is a quoted key
/// there, where a dotted `-c` key would split it. A table given whole replaces
/// whatever earlier `-c`s set under the same key, so these come before Luna's
/// own `mcp_servers.luna.*` (`mcp_args`) and the library's `plugins.*`
/// (background/command.rs). A server no layer defines would fail the run
/// ("invalid transport"): only those read are named.
///
/// A server a layer calls `luna` is no exception: off, unless Luna gives the
/// run its own (`luna_supplied`, `mcp_args`). Then Codex merges the two key by
/// key, so the layers must set nothing Luna's does not replace
/// (`LUNA_SERVER_KEYS`), or the helper is not started: what runs as `luna` is
/// Luna's server, exactly.
pub(crate) fn connectors_off(mcp: &crate::codex::config::HelperMcp, luna_supplied: bool) -> Result<Vec<String>, String> {
    let mut args = vec!["-c".to_string(), "features.apps=false".to_string()];
    let switch = |on: bool| {
        let mut t = toml_edit::InlineTable::new();
        t.insert("enabled", on.into());
        toml_edit::Value::InlineTable(t)
    };
    let mut servers = toml_edit::InlineTable::new();
    for (name, keys) in &mcp.servers {
        if name != LUNA_SERVER || !luna_supplied {
            servers.insert(name, switch(false));
            continue;
        }
        let other: Vec<&str> = keys.iter().map(String::as_str).filter(|k| !LUNA_SERVER_KEYS.contains(k)).collect();
        if !other.is_empty() {
            return Err(format!(
                "the helper is not started: its config defines an MCP server named {LUNA_SERVER} whose {} would be merged \
                 into Luna's own; rename that server, or spawn with connectors: true",
                other.join(", ")
            ));
        }
        // What it sets Luna's overrides replace, but for one that turns it off.
        servers.insert(LUNA_SERVER, switch(true));
    }
    if !servers.is_empty() {
        args.extend(["-c".to_string(), format!("mcp_servers={servers}")]);
    }
    let mut plugins = toml_edit::InlineTable::new();
    for id in &mcp.plugins {
        plugins.insert(id, switch(false));
    }
    plugins.insert("_default", switch(false));
    args.extend(["-c".to_string(), format!("plugins={plugins}")]);
    Ok(args)
}

/// `connectors_off`'s overrides as applied: what a helper is kept to.
pub(crate) fn connectors_off_applied(off: &[String]) -> Applied {
    Item::HelperMcpAlone.with(off.iter().filter(|a| *a != "-c").map(|v| format!("-c {v}")).collect::<Vec<_>>().join(", "))
}

/// The name Luna's own MCP server goes by in a Codex session (`mcp_args`).
const LUNA_SERVER: &str = "luna";

/// The keys of `[mcp_servers.luna]` that `mcp_args` sets, and `enabled`, which
/// `connectors_off` does: an account's server of that name setting only these
/// is replaced by Luna's whole.
const LUNA_SERVER_KEYS: [&str; 5] = ["url", "bearer_token_env_var", "tool_timeout_sec", "default_tools_approval_mode", "enabled"];

/// Luna's name for Codex's `--approve-for-me`: what Codex would ask goes to a
/// reviewer model. It takes no `--ask-for-approval` or `--sandbox` and runs
/// in `workspace-write`.
pub(crate) const AUTO_REVIEW: &str = "auto-review";
pub(crate) const AUTO_REVIEW_SANDBOX: &str = "workspace-write";

/// Codex puts the id of the thread it is on in the window title, which no pane
/// shows (`PtyManager::codex_thread`): what follows it through a `/new`, a
/// `/resume` or a `/fork` in its TUI, and tells two chats in one folder apart
/// before either has a rollout (codex/session/rollouts.rs `rollout_for`).
const CODEX_TITLE: &str = "tui.terminal_title=[\"thread-id\"]";

/// Luna's own sandbox value: Codex's read-only with the network on, which
/// `sandbox_mode` cannot say (see `profile_args`).
pub(crate) const READ_ONLY_NET: &str = "read-only-net";

/// Whether a session in `sandbox` is kept from Luna's `core/`, where the
/// core's socket is off Windows (`profile_args`): a second wall, as the core
/// refuses a sandboxed client itself (peer.rs) and nothing in the folder lets
/// anyone in (ADR-0021); `windows`, whether it runs there. The read-only
/// sandboxes only: Codex's `:workspace`
/// profile drops what an account's `[sandbox_workspace_write]` adds, its roots
/// and its network, so `workspace-write` stays Codex's own mode, without the
/// deny. And not on Windows, where Codex's unelevated sandbox refuses every
/// command under a profile with a deny (0.167.0: "requires the elevated
/// Windows sandbox backend"); its write-restricted token reads whatever the
/// user can, `core/` under its private DACL included (threat model F3).
pub(crate) fn denies_core(sandbox: &str, windows: bool) -> bool {
    !windows && matches!(sandbox, "read-only" | READ_ONLY_NET)
}

/// Whether a session in `sandbox` runs under a profile of Luna's
/// (`profile_args`) rather than under Codex's own mode.
pub(crate) fn takes_profile(sandbox: &str, windows: bool) -> bool {
    sandbox == READ_ONLY_NET || denies_core(sandbox, windows)
}

/// A read-only sandbox as `-c` overrides: a permission profile of Luna's that
/// extends Codex's `:read-only`, with direct network for `read-only-net`
/// (which `sandbox_mode` cannot say) and with `deny`, Luna's `core/`, out of
/// reach, nothing written to the account's config.toml. Codex's docs say not
/// to set `sandbox_mode` beside `default_permissions`; this override beats one
/// in the account's or the project's config, so the disk stays read-only
/// either way. The name, `profile`, is new on every start (pty/start.rs
/// `codex_profile`): Codex lays the rules any config has for a profile's name
/// over the ones given here, so a project's `.codex/config.toml` saying
/// `":root" = "write"` for a name it could know would make this writable. It
/// need not last: a resume takes a new one. On Windows TLS through schannel
/// still fails under the sandbox's restricted token, as it does in
/// `workspace-write` with its network on (see `git_ssl_backend_envs`,
/// luna_guide.md).
pub(crate) fn profile_args(profile: &str, network: bool, deny: Option<&str>) -> Vec<String> {
    let mut values = vec![format!("default_permissions=\"{profile}\""), format!("permissions.{profile}.extends=\":read-only\"")];
    if network {
        values.push(format!("permissions.{profile}.network.enabled=true"));
    }
    if let Some(dir) = deny {
        // The path in the value, as a TOML string: Codex splits a `-c` key at
        // every dot, and a path may hold one.
        values.push(format!("permissions.{profile}.filesystem={{{}=\"deny\"}}", toml_edit::Value::from(dir)));
    }
    values.into_iter().flat_map(|value| ["-c".to_string(), value]).collect()
}

/// What `read-only-net` needs for git on Windows, whose https goes through
/// schannel by default and fails under the sandbox's token: entries for
/// `GIT_CONFIG_*`, after any it was started with. Nothing if those already
/// choose a backend, or if their count is one git itself would refuse.
pub(crate) fn git_ssl_backend_envs(count: Option<&str>, key: impl Fn(usize) -> Option<String>) -> Vec<(String, String)> {
    let n: usize = match count {
        None => 0,
        Some(c) => match c.trim().parse() {
            Ok(n) => n,
            Err(_) => return Vec::new(),
        },
    };
    if (0..n).any(|i| key(i).is_some_and(|k| k.eq_ignore_ascii_case("http.sslBackend"))) {
        return Vec::new();
    }
    vec![
        ("GIT_CONFIG_COUNT".to_string(), (n + 1).to_string()),
        (format!("GIT_CONFIG_KEY_{n}"), "http.sslBackend".to_string()),
        (format!("GIT_CONFIG_VALUE_{n}"), "openssl".to_string()),
    ]
}

/// A sandbox as `-c` overrides, for a helper's `codex app-server`; one that
/// takes Luna's profile by its name, `profile`, `deny` kept from it.
pub(crate) fn sandbox_args(sandbox: &str, profile: Option<&str>, deny: Option<&str>) -> Vec<String> {
    match profile.filter(|_| takes_profile(sandbox, cfg!(windows))) {
        Some(profile) => profile_args(profile, sandbox == READ_ONLY_NET, deny),
        None => vec!["-c".into(), format!("sandbox_mode=\"{sandbox}\"")],
    }
}

/// The override that gives Codex its Windows sandbox, for the modes that
/// need one. Without it Codex refuses every command under `read-only` and
/// `workspace-write`, a `Get-Content` included. The unelevated kind runs each
/// command with a restricted token: no administrator, no extra users; under
/// `workspace-write` Codex grants its sandbox write access to the folder
/// once, in the folder's ACL.
/// Codex's own sandbox (`OWN`) may be any, so it gets one too: unused where
/// that turns the sandbox off. Luna's choice where the account names none,
/// for Codex to run its commands at all: what the sandbox holds is Codex's
/// own, not Luna's.
pub(crate) fn codex_windows_sandbox(sandbox: &str) -> Option<String> {
    (cfg!(windows) && matches!(sandbox, "read-only" | READ_ONLY_NET | "workspace-write" | OWN))
        .then(|| "windows.sandbox=\"unelevated\"".to_string())
}

/// PATH for a Codex session in a Windows sandbox: `path` without
/// `…\Microsoft\WindowsApps`. Codex takes the first `pwsh.exe` on PATH for its
/// shell, and the Store PowerShell's there is an app alias the restricted
/// token cannot start — `CreateProcessAsUserW failed: 5` on every command,
/// whatever it was. Without it Codex falls back to a PowerShell installed the
/// ordinary way, or to Windows PowerShell; Store apps' aliases are out of the
/// sandbox's reach either way.
pub(crate) fn without_windows_apps(path: &str) -> String {
    path.split(';').filter(|p| !p.to_lowercase().replace('/', "\\").contains("\\microsoft\\windowsapps")).collect::<Vec<_>>().join(";")
}

/// Luna's MCP server at `url` as `-c` overrides for Codex, the token through
/// `LUNA_MCP_TOKEN`: neither touches the account's config.toml.
pub(crate) fn mcp_args(url: &str) -> Vec<String> {
    let mut args = Vec::new();
    for value in [
        format!("mcp_servers.luna.url=\"{url}\""),
        "mcp_servers.luna.bearer_token_env_var=\"LUNA_MCP_TOKEN\"".into(),
        // Codex gives an MCP call 60 s by default.
        format!("mcp_servers.luna.tool_timeout_sec={}", MCP_TOOL_TIMEOUT_MS / 1000),
        // And outside the full bypass it may stop to ask before a server's
        // tool runs; `approve` lets Luna's through, as `--allowedTools` does
        // for Claude Code.
        "mcp_servers.luna.default_tools_approval_mode=\"approve\"".into(),
    ] {
        args.push("-c".into());
        args.push(value);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::launch::claude_settings;

    /// What a `-c` table value turns on and off, read back as Codex parses
    /// the value: name and `enabled`.
    fn switched(arg: &str, key: &str) -> Vec<(String, bool)> {
        let value = arg.strip_prefix(&format!("{key}=")).unwrap_or_else(|| panic!("{arg}"));
        let doc: toml_edit::DocumentMut = format!("t = {value}").parse().unwrap();
        doc["t"]
            .as_inline_table()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.to_string(), v.as_inline_table().unwrap().get("enabled").unwrap().as_bool().unwrap()))
            .collect()
    }

    #[test]
    fn a_helper_has_every_server_and_plugin_of_its_run_off() {
        use crate::codex::config::HelperMcp;
        let mcp = |n: &[(&str, &[&str])], plugins: &[&str]| HelperMcp {
            servers: n.iter().map(|(name, keys)| (name.to_string(), keys.iter().map(|k| k.to_string()).collect())).collect(),
            plugins: plugins.iter().map(|p| p.to_string()).collect(),
        };
        let off = |name: &str| (name.to_string(), false);
        // Nothing named anywhere: apps off, and plugins only ChatGPT installed.
        let args = connectors_off(&mcp(&[], &[]), false).unwrap();
        assert_eq!(args[..3], ["-c", "features.apps=false", "-c"]);
        assert_eq!(switched(&args[3], "plugins"), [off("_default")]);
        assert_eq!(args.len(), 4);
        // Every name, odd ones included: a quoted key in the table, never split.
        let names = ["gmail", "team.slack", "café", "", "a=b"];
        let servers: Vec<(&str, &[&str])> = names.iter().map(|n| (*n, &["url"][..])).collect();
        for supplied in [false, true] {
            let args = connectors_off(&mcp(&servers, &["crm@market", "files.v2@corp"]), supplied).unwrap();
            assert_eq!(switched(&args[3], "mcp_servers"), names.map(off));
            assert_eq!(switched(&args[5], "plugins"), [off("crm@market"), off("files.v2@corp"), off("_default")]);
        }
        // A `luna` of the layers': off when Luna gives the run no server of its own.
        let theirs = mcp(&[("luna", &["command", "args"])], &[]);
        assert_eq!(switched(&connectors_off(&theirs, false).unwrap()[3], "mcp_servers"), [off("luna")]);
        // With Luna's own, only one Luna's would replace whole goes, turned on.
        let e = connectors_off(&theirs, true).unwrap_err();
        assert!(e.contains("not started") && e.contains("command, args"), "{e}");
        let like_lunas = mcp(&[("luna", &["url", "enabled"])], &[]);
        assert_eq!(switched(&connectors_off(&like_lunas, true).unwrap()[3], "mcp_servers"), [("luna".to_string(), true)]);
        // What `mcp_args` sets is what the layers' may set.
        for arg in mcp_args("http://h/mcp").iter().filter(|a| *a != "-c") {
            let key = arg.trim_start_matches("mcp_servers.luna.").split('=').next().unwrap();
            assert!(LUNA_SERVER_KEYS.contains(&key), "{key}");
        }
    }

    /// A chat's settings carry its hooks and the fullscreen renderer; a
    /// helper's, which has no screen, neither; nothing else, the core's deny.
    #[test]
    fn a_chats_settings_carry_its_hooks_and_the_renderer_a_helpers_do_not() {
        let library = crate::library::SessionSkills {
            hidden: vec!["deploy".to_string()],
            excluded: vec!["E:/acc/work/CLAUDE.md".to_string()],
            ..Default::default()
        };
        let chat = claude_settings(Some("http://h/hook/c/s"), &library, "/d/core", None);
        let pre = chat["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0]["matcher"], "AskUserQuestion");
        assert!(chat["hooks"]["Stop"].is_array() && chat["hooks"]["SubagentStop"].is_array());
        assert_eq!(chat["tui"], "fullscreen");
        assert_eq!(chat["skillOverrides"]["deploy"], "off");
        assert_eq!(chat["claudeMdExcludes"], serde_json::json!(["E:/acc/work/CLAUDE.md"]));
        let none = crate::library::SessionSkills::default();
        let helper = claude_settings(None, &none, "/d/core", Some(crate::claude::launch::HelperSettings { connectors: false }));
        assert!(helper.get("hooks").is_none(), "no hooks of its own: its stream says it all");
        assert!(helper.get("tui").is_none(), "headless: nothing to draw");
        let login = claude_settings(None, &none, "/d/core", None);
        assert_eq!(login.keys().collect::<Vec<_>>(), ["permissions", "tui"], "the core's deny and the renderer");
    }

    /// Settings › Chats › Copy on select, at every start and never into the
    /// account: off is `never`, the argv as before the setting; on is
    /// `always`; as the account has it passes no key at all. Notifications
    /// stay off whichever it is.
    #[test]
    fn a_chats_copy_on_select_follows_the_setting() {
        let argv = |copy| {
            let start = ChatStart {
                id: "c1",
                folder: "C:/p",
                account_path: "C:/acc",
                resume: None,
                trust: None,
                own_windows_sandbox: false,
                worktree: false,
                model: None,
                effort: "low",
                approval: OWN,
                sandbox: OWN,
                profile: None,
                deny: None,
                sandbox_path: None,
                git_envs: &[],
                mcp: None,
                hooks: None,
                skill_args: &[],
                copy_on_select: copy,
            };
            let (args, _, applied) = chat_args(&start);
            let shown = applied.iter().find(|a| a.key == "copy-on-select").map(|a| a.value.clone());
            let copies: Vec<String> = args.iter().filter(|a| a.starts_with("tui.copy_on_select=")).cloned().collect();
            assert!(args.windows(2).any(|w| w == ["-c", TUI_NOTIFICATIONS]), "{args:?}");
            (copies, shown)
        };
        let off = argv(CopyOnSelect::Off);
        assert_eq!(off, (vec!["tui.copy_on_select=\"never\"".to_string()], Some("-c tui.copy_on_select=\"never\"".to_string())));
        let on = argv(CopyOnSelect::On);
        assert_eq!(on, (vec!["tui.copy_on_select=\"always\"".to_string()], Some("-c tui.copy_on_select=\"always\"".to_string())));
        assert_eq!(argv(CopyOnSelect::Account), (vec![], None));
    }

    /// A chat with the hub's listener up gets Luna's status hooks, trusted by
    /// their `hooks.state`, and their URL only in its environment.
    #[test]
    fn a_chat_gets_its_status_hooks_and_their_url_out_of_its_command_line() {
        let start = |hooks| ChatStart {
            id: "c1",
            folder: "C:/p",
            account_path: "C:/acc",
            resume: None,
            trust: None,
            own_windows_sandbox: false,
            worktree: false,
            model: None,
            effort: "low",
            approval: OWN,
            sandbox: OWN,
            profile: None,
            deny: None,
            sandbox_path: None,
            git_envs: &[],
            mcp: None,
            hooks,
            skill_args: &[],
            copy_on_select: CopyOnSelect::Off,
        };
        let url = "http://127.0.0.1:9/hook/c1/0123456789abcdef0123456789abcdef";
        let (args, envs, applied) = chat_args(&start(Some((url, "/usr/bin/curl"))));
        assert!(args.windows(2).any(|w| w[0] == "-c" && w[1].starts_with("hooks.state=")));
        assert!(args.iter().any(|a| a.starts_with("hooks.PermissionRequest=")));
        assert!(!args.iter().any(|a| a.contains("0123456789abcdef")), "the secret is in no argument");
        assert!(envs.contains(&(crate::codex::hooks::HOOK_URL_ENV, url.to_string())));
        assert!(!applied.iter().any(|a| format!("{a:?}").contains("0123456789abcdef")), "nor in what Luna says it applied");
        for (event, _, _) in crate::codex::hooks::EVENTS {
            assert!(HOOKS_VALUE.contains(&format!("-c hooks.{event},")) || HOOKS_VALUE.contains(&format!("-c hooks.{event} ")), "{event}");
        }
        let (args, envs, _) = chat_args(&start(None));
        assert!(!args.iter().any(|a| a.starts_with("hooks.")), "no listener, no hooks");
        assert!(!envs.iter().any(|(k, _)| *k == crate::codex::hooks::HOOK_URL_ENV));
    }

    /// The modes a chat keeps, as the window saves them, to the flags Codex
    /// is started with, through the start's own reading of them (pty/start.rs
    /// `codex_modes`, which `ensure_codex_session` builds its `ChatStart`
    /// from): Codex's own gets none, the closed pair goes as itself (a profile
    /// of Luna's keeping `core/` out where Codex can, off Windows), and the
    /// full bypass only for exactly never + danger-full-access. A start and a
    /// resume of the same chat, as after a core replaced, pass the same.
    #[test]
    fn a_chats_modes_are_flags_only_where_it_names_them() {
        let argv = |approval: &str, sandbox: &str, resume: Option<&str>| {
            let m = crate::pty::codex_modes(approval, sandbox);
            let start = ChatStart {
                id: "c1",
                folder: "C:/p",
                account_path: "C:/acc",
                resume,
                trust: None,
                own_windows_sandbox: false,
                worktree: false,
                model: None,
                effort: "low",
                approval: &m.approval,
                sandbox: &m.sandbox,
                profile: m.profile.as_deref(),
                deny: m.deny.as_deref(),
                sandbox_path: m.sandbox_path.as_deref(),
                git_envs: &m.git_envs,
                mcp: None,
                hooks: None,
                skill_args: &[],
                copy_on_select: CopyOnSelect::Off,
            };
            let args = chat_args(&start).0;
            let flag = |f: &str| args.iter().position(|a| a == f).map(|i| args.get(i + 1).cloned().unwrap_or_default());
            // A sandbox held by Luna's profile reads as the profile and what it denies.
            let profile = args.iter().any(|a| a.starts_with("default_permissions=\"luna-"));
            let denies = args.iter().any(|a| a.contains(".filesystem={") && a.contains("\"deny\""));
            let sandbox = match (flag("--sandbox"), profile) {
                (Some(s), false) => Some(s),
                (None, true) => Some(format!("profile{}", if denies { ", core denied" } else { "" })),
                (None, false) => None,
                (Some(s), true) => panic!("--sandbox {s} beside a profile"),
            };
            (
                flag("--ask-for-approval"),
                sandbox,
                args.iter().any(|a| a == "--dangerously-bypass-approvals-and-sandbox"),
                args.iter().any(|a| a == "--approve-for-me"),
            )
        };
        let s = |v: &str| Some(v.to_string());
        let read_only = if cfg!(windows) { s("read-only") } else { s("profile, core denied") };
        let read_only_net = if cfg!(windows) { s("profile") } else { s("profile, core denied") };
        for (asked, want) in [
            ((OWN, OWN), (None, None, false, false)),
            (("on-request", "read-only"), (s("on-request"), read_only.clone(), false, false)),
            (("never", "danger-full-access"), (None, None, true, false)),
            (("never", OWN), (s("never"), None, false, false)),
            ((OWN, "danger-full-access"), (None, s("danger-full-access"), false, false)),
            (("on-request", "danger-full-access"), (s("on-request"), s("danger-full-access"), false, false)),
            (("on-request", READ_ONLY_NET), (s("on-request"), read_only_net, false, false)),
            ((OWN, "workspace-write"), (None, s("workspace-write"), false, false)),
            // `--approve-for-me` brings its sandbox, whatever the chat saved.
            (("auto-review", OWN), (None, None, false, true)),
            (("auto-review", "danger-full-access"), (None, None, false, true)),
            // A word Luna does not know is the closed pair, never the bypass.
            (("", ""), (s("on-request"), read_only.clone(), false, false)),
            (("untrusted", "danger-full-access"), (s("on-request"), read_only.clone(), false, false)),
        ] {
            assert_eq!(argv(asked.0, asked.1, None), want, "{asked:?}");
            assert_eq!(argv(asked.0, asked.1, Some("019a0000-0000-7000-8000-000000000000")), want, "{asked:?}, resumed");
        }
    }

    #[test]
    fn read_only_net_is_a_profile_and_never_a_sandbox_mode() {
        let args = crate::pty::codex_sandbox_args(READ_ONLY_NET);
        assert!(!args.iter().any(|a| a.starts_with("sandbox_mode")), "Codex's docs say not to set both");
        let profile = args[1].strip_prefix("default_permissions=\"").and_then(|p| p.strip_suffix('"')).expect("the profile is named first");
        assert!(args.contains(&format!("permissions.{profile}.extends=\":read-only\"")));
        assert!(args.contains(&format!("permissions.{profile}.network.enabled=true")));
        assert_eq!(args.iter().filter(|a| *a == "-c").count(), if cfg!(windows) { 3 } else { 4 }, "and the core's deny off Windows");
        assert_ne!(crate::pty::codex_sandbox_args(READ_ONLY_NET)[1], args[1], "no config can have rules ready for the next one's name");
        if cfg!(windows) {
            assert_eq!(crate::pty::codex_sandbox_args("read-only"), ["-c", "sandbox_mode=\"read-only\""]);
        }
        assert_eq!(codex_windows_sandbox(READ_ONLY_NET), codex_windows_sandbox("read-only"), "it runs in the same sandbox");
    }

    /// The folder a profile's `-c` overrides deny, read back as Codex parses
    /// the value; None without one.
    fn denied(args: &[String]) -> Option<String> {
        let value = args.iter().find_map(|a| a.split_once(".filesystem=").map(|(_, v)| v))?;
        let doc: toml_edit::DocumentMut = format!("filesystem = {value}").parse().unwrap();
        let entries: Vec<_> =
            doc["filesystem"].as_inline_table().unwrap().iter().map(|(k, v)| (k.to_string(), v.as_str().map(str::to_owned))).collect();
        match entries.as_slice() {
            [(dir, Some(access))] if access == "deny" => Some(dir.clone()),
            other => panic!("one folder denied, not {other:?}"),
        }
    }

    #[test]
    fn the_read_only_sandboxes_are_kept_out_of_the_core_folder_but_on_windows() {
        for windows in [false, true] {
            for sandbox in ["read-only", READ_ONLY_NET] {
                assert_eq!(denies_core(sandbox, windows), !windows, "{sandbox}, windows {windows}");
                assert_eq!(takes_profile(sandbox, windows), !windows || sandbox == READ_ONLY_NET, "{sandbox}, windows {windows}");
            }
            for sandbox in ["workspace-write", "danger-full-access"] {
                assert!(!denies_core(sandbox, windows) && !takes_profile(sandbox, windows), "{sandbox}: Codex's own mode");
            }
        }
        // A path with what a `-c` key or TOML would split on comes back whole.
        for dir in ["/home/u/.local/share/lu.na/core", r"C:\Users\a.b\Lu na\core", "/x/\"q\"/core"] {
            let args = profile_args("luna-p", false, Some(dir));
            assert!(args.iter().any(|a| a.starts_with("permissions.luna-p.filesystem={")), "{args:?}");
            assert_eq!(denied(&args).as_deref(), Some(dir));
        }
        assert_eq!(denied(&profile_args("luna-p", true, None)), None);
        // This machine's own: Luna's `core/`, wherever Codex can keep it out.
        let core = crate::ipc::core_dir().to_string_lossy().into_owned();
        for sandbox in ["read-only", READ_ONLY_NET, "workspace-write", "danger-full-access"] {
            let want = denies_core(sandbox, cfg!(windows)).then(|| core.clone());
            assert_eq!(denied(&crate::pty::codex_sandbox_args(sandbox)), want, "{sandbox}");
        }
    }

    #[test]
    fn git_takes_its_backend_after_entries_it_already_has() {
        let none = |_: usize| None;
        let added = git_ssl_backend_envs(None, none);
        assert_eq!(
            added,
            [
                ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
                ("GIT_CONFIG_KEY_0".to_string(), "http.sslBackend".to_string()),
                ("GIT_CONFIG_VALUE_0".to_string(), "openssl".to_string()),
            ]
        );
        let own = |i: usize| (i == 0).then(|| "user.name".to_string());
        let added = git_ssl_backend_envs(Some("1"), own);
        assert_eq!(added[0].1, "2");
        assert_eq!(added[1].0, "GIT_CONFIG_KEY_1", "the one it has stays as it is");
        let chosen = |i: usize| (i == 1).then(|| "HTTP.sslbackend".to_string());
        assert!(git_ssl_backend_envs(Some("2"), chosen).is_empty(), "a backend chosen already stays chosen");
        assert!(git_ssl_backend_envs(Some("many"), none).is_empty(), "a count git refuses is not made worse");
        assert!(crate::pty::codex_git_envs("read-only").is_empty(), "only where the network is on");
    }

    #[test]
    fn the_sandbox_path_has_no_store_aliases() {
        let path = r"C:\Users\n\AppData\Local\Microsoft\WindowsApps\Microsoft.PowerShell_8wekyb3d8bbwe;C:\WINDOWS\system32;C:\Users\n\AppData\Local\Microsoft\WindowsApps;C:\Program Files\PowerShell\7";
        assert_eq!(without_windows_apps(path), r"C:\WINDOWS\system32;C:\Program Files\PowerShell\7");
        assert_eq!(crate::pty::codex_sandbox_path("danger-full-access"), None, "full access keeps the PATH it had");
    }
}
