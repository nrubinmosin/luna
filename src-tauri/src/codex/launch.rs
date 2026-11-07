//! How a Codex session in a pane is started, and what a helper's `codex exec`
//! shares with it (background/): arguments and environment, laid out from
//! what the start has already settled. Pure: nothing here reads the
//! environment, the disk or Luna's state, logs, writes, registers or mints.
//! The start does that under the chat's spawn gate (pty/start.rs) and hands
//! the results in; a helper's turn, in background/command.rs.

use crate::pty::MCP_TOOL_TIMEOUT_MS;

/// A Codex chat's start as `chat_args` lays it out: what the window asked
/// for, and what the start settled for it under the spawn gate — the trust
/// override, the sandbox's profile, PATH and git entries, the guard, Luna's
/// server and the token — which this only passes on.
pub(crate) struct ChatStart<'a> {
    pub(crate) id: &'a str,
    pub(crate) folder: &'a str,
    pub(crate) account_path: &'a str,
    pub(crate) resume: Option<&'a str>,
    pub(crate) trust: Option<[String; 2]>,
    pub(crate) worktree: bool,
    pub(crate) model: Option<&'a str>,
    pub(crate) effort: &'a str,
    pub(crate) approval: &'a str,
    /// The sandbox it runs in: `AUTO_REVIEW_SANDBOX` under the reviewer.
    pub(crate) sandbox: &'a str,
    /// The `read-only-net` profile's name, new on every start
    /// (`read_only_net_args`); in any other sandbox, None.
    pub(crate) profile: Option<&'a str>,
    /// PATH for the sandbox (pty/start.rs `codex_sandbox_path`), where it
    /// takes one.
    pub(crate) sandbox_path: Option<&'a str>,
    /// What the sandbox needs of git's config (pty/start.rs `codex_git_envs`).
    pub(crate) git_envs: &'a [(&'static str, String)],
    pub(crate) hooks_unasked: bool,
    /// The guard against endless waits (guard.rs) as its `-c` override, if
    /// the session has it (pty/start.rs `codex_guard_args`).
    pub(crate) guard: &'a [String],
    pub(crate) parent: Option<&'a str>,
    /// Luna's MCP server's URL and the session's token, while the server is
    /// up to take it.
    pub(crate) mcp: Option<(&'a str, &'a str)>,
    pub(crate) skill_args: &'a [String],
    pub(crate) prompt: Option<&'a str>,
}

/// The arguments and environment of a Codex chat's start; `codex login`
/// takes none of them.
pub(crate) fn chat_args(s: &ChatStart) -> (Vec<String>, Vec<(&'static str, String)>) {
    let mut args: Vec<String> = vec![];
    let mut envs = vec![("CODEX_HOME", s.account_path.to_string())];
    if let Some(session_id) = s.resume {
        args.push("resume".into());
        args.push(session_id.to_string());
    }
    args.push("-C".into());
    args.push(s.folder.to_string());
    // The session runs in this process, in Luna's pty, never in Codex's
    // shared background server: Luna stops, deletes and counts it as a
    // process of its own. The `-c` overrides below need that anyway;
    // saying so keeps Codex from warning about it on every start.
    args.push("--no-daemon".into());
    if let Some(trust) = &s.trust {
        args.extend(trust.iter().cloned());
    }
    // A resumed thread already has its worktree; Codex refuses the
    // flag on `resume`.
    if s.worktree && s.resume.is_none() {
        args.push("--worktree".into());
    }
    if let Some(m) = s.model {
        args.push("--model".into());
        args.push(m.to_string());
    }
    args.push("-c".into());
    args.push(format!("model_reasoning_effort=\"{}\"", s.effort));
    let (approval, sandbox) = (s.approval, s.sandbox);
    // The pair Codex itself spells as one flag.
    if approval == "never" && sandbox == "danger-full-access" {
        args.push("--dangerously-bypass-approvals-and-sandbox".into());
    } else {
        if approval == AUTO_REVIEW {
            args.push("--approve-for-me".into());
        } else {
            args.push("--ask-for-approval".into());
            args.push(approval.to_string());
            args.push("-c".into());
            args.push(CODEX_PERSON_REVIEWS.into());
            // Without its profile, `--sandbox read-only-net`, which Codex
            // refuses: never a sandbox looser than the one asked for.
            match s.profile.filter(|_| sandbox == READ_ONLY_NET) {
                Some(profile) => args.extend(read_only_net_args(profile)),
                None => {
                    args.push("--sandbox".into());
                    args.push(sandbox.to_string());
                }
            }
        }
        if let Some(sb) = codex_windows_sandbox(sandbox) {
            args.push("-c".into());
            args.push(sb);
        }
        if let Some(path) = s.sandbox_path {
            envs.push(("PATH", path.to_string()));
        }
        envs.extend(s.git_envs.iter().cloned());
    }
    if s.hooks_unasked {
        args.push(HOOKS_UNASKED.into());
    }
    args.extend(s.guard.iter().cloned());
    args.push("-c".into());
    args.push(CODEX_TITLE.into());
    envs.extend(crate::pty::session_envs(s.id, s.parent));
    if let Some((url, token)) = s.mcp {
        args.extend(mcp_args(url));
        envs.push(("LUNA_MCP_TOKEN", token.to_string()));
    }
    args.extend(s.skill_args.iter().cloned());
    if let Some(p) = s.prompt.map(str::trim).filter(|p| !p.is_empty()) {
        // A prompt that starts with a dash is still a prompt.
        args.push("--".into());
        args.push(p.to_string());
    }
    (args, envs)
}

/// `hooks.PreToolUse` for Codex, posting each shell call to `url` through
/// `curl` (pty/start.rs `codex_guard_args`). A TOML literal string: the
/// command's Windows paths hold backslashes.
pub(crate) fn guard_override(url: &str, curl: &str) -> String {
    format!(r#"hooks.PreToolUse=[{{matcher="Bash",hooks=[{{type="command",command='{}',timeout=5}}]}}]"#, hook_command(url, curl))
}

/// `hook_command` for Codex, which runs a hook through the user's shell —
/// PowerShell on Windows, where a bare `@-` does not parse (0.166: the guard's
/// hook failed there, and a failing hook lets the call go ahead) — or through
/// `cmd.exe /C` when it has none, with an environment of its own. So `"@-"`
/// quoted, which PowerShell, cmd and sh all hand curl as `@-`, and curl by its
/// full path, found whatever PATH the hook gets.
fn hook_command(url: &str, curl: &str) -> String {
    format!(r#"{curl} -s -m 3 -X POST --data-binary "@-" {url}"#)
}

/// Luna's name for Codex's `--approve-for-me`: what Codex would ask goes to a
/// reviewer model. It takes no `--ask-for-approval` or `--sandbox` and runs
/// in `workspace-write`.
pub(crate) const AUTO_REVIEW: &str = "auto-review";
pub(crate) const AUTO_REVIEW_SANDBOX: &str = "workspace-write";
/// What Codex asks goes to a person, whatever an account's or project's
/// `approvals_reviewer` says: `auto_review` there would have a reviewer model
/// answer a session Luna started, and held its helper to, as asking one.
pub(crate) const CODEX_PERSON_REVIEWS: &str = "approvals_reviewer=\"user\"";
/// Codex runs the project's hooks without asking for trust (codex/hooks.rs).
pub(crate) const HOOKS_UNASKED: &str = "--dangerously-bypass-hook-trust";
/// Codex puts the id of the thread it is on in the window title, which no pane
/// shows (`PtyManager::codex_thread`): what follows it through a `/new`, a
/// `/resume` or a `/fork` in its TUI, and tells two chats in one folder apart
/// before either has a rollout (codex/session/rollouts.rs `rollout_for`).
const CODEX_TITLE: &str = "tui.terminal_title=[\"thread-id\"]";

/// Luna's own sandbox value: Codex's read-only with the network on, which
/// `sandbox_mode` cannot say (see `read_only_net_args`).
pub(crate) const READ_ONLY_NET: &str = "read-only-net";

/// `read-only-net` as `-c` overrides: a permission profile of Luna's that
/// extends Codex's `:read-only`, with direct network, and nothing written to
/// the account's config.toml. Codex's docs say not to set `sandbox_mode` beside
/// `default_permissions`; this override beats one in the account's or the
/// project's config, so the disk stays read-only either way. The name,
/// `profile`, is new on every start (pty/start.rs `codex_profile`): Codex lays
/// the rules any config has for a profile's name over the ones given here, so
/// a project's `.codex/config.toml` saying `":root" = "write"` for a name it
/// could know would make this writable. It need not last: a resume takes a new
/// one. On Windows TLS through schannel still fails under the sandbox's
/// restricted token, as it does in `workspace-write` with its network on (see
/// `git_ssl_backend_envs`, luna_guide.md).
pub(crate) fn read_only_net_args(profile: &str) -> Vec<String> {
    [
        format!("default_permissions=\"{profile}\""),
        format!("permissions.{profile}.extends=\":read-only\""),
        format!("permissions.{profile}.network.enabled=true"),
    ]
    .into_iter()
    .flat_map(|value| ["-c".to_string(), value])
    .collect()
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

/// A sandbox as `-c` overrides, for `codex exec`, which asks nobody;
/// `read-only-net` by its `profile`.
pub(crate) fn sandbox_args(sandbox: &str, profile: Option<&str>) -> Vec<String> {
    match profile.filter(|_| sandbox == READ_ONLY_NET) {
        Some(profile) => read_only_net_args(profile),
        None => vec!["-c".into(), format!("sandbox_mode=\"{sandbox}\"")],
    }
}

/// The override that gives Codex its Windows sandbox, for the modes that
/// need one. Without it Codex refuses every command under `read-only` and
/// `workspace-write`, a `Get-Content` included. The unelevated kind runs each
/// command with a restricted token: no administrator, no extra users; under
/// `workspace-write` Codex grants its sandbox write access to the folder
/// once, in the folder's ACL.
pub(crate) fn codex_windows_sandbox(sandbox: &str) -> Option<String> {
    (cfg!(windows) && matches!(sandbox, "read-only" | READ_ONLY_NET | "workspace-write"))
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

    /// A chat's settings carry its hooks and, with the guard, a second
    /// PreToolUse entry on the shell tools; a background helper's, the guard
    /// alone; nothing at all, none.
    #[test]
    fn the_guard_rides_with_the_hooks_or_alone() {
        let library = crate::library::SessionSkills {
            hidden: vec!["deploy".to_string()],
            excluded: vec!["E:/acc/work/CLAUDE.md".to_string()],
            ..Default::default()
        };
        let chat = claude_settings(Some("http://h/hook/c/s"), Some("http://h/guard/c/s"), &library);
        let pre = chat["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 2);
        assert_eq!(pre[0]["matcher"], "AskUserQuestion");
        assert_eq!(pre[1]["matcher"], "Bash|PowerShell|Monitor");
        assert!(pre[1]["hooks"][0]["command"].as_str().unwrap().ends_with("http://h/guard/c/s"));
        assert!(chat["hooks"]["Stop"].is_array() && chat["hooks"]["SubagentStop"].is_array());
        assert_eq!(chat["skillOverrides"]["deploy"], "off");
        assert_eq!(chat["claudeMdExcludes"], serde_json::json!(["E:/acc/work/CLAUDE.md"]));
        let none = crate::library::SessionSkills::default();
        let unguarded = claude_settings(Some("http://h/hook/c/s"), None, &none);
        assert_eq!(unguarded["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        let helper = claude_settings(None, Some("http://h/guard/c/s"), &none);
        let hooks = helper["hooks"].as_object().unwrap();
        assert_eq!(hooks.keys().collect::<Vec<_>>(), ["PreToolUse"], "no hooks of its own: its stream says it all");
        assert!(claude_settings(None, None, &none).is_empty());
        let curl = crate::pty::curl_program();
        let guard = guard_override("http://h/guard/c/s", &curl);
        assert!(guard.starts_with(r#"hooks.PreToolUse=[{matcher="Bash",hooks=[{type="command",command='"#), "{guard}");
        assert!(guard.ends_with(r#" -s -m 3 -X POST --data-binary "@-" http://h/guard/c/s',timeout=5}]}]"#), "{guard}");
        assert!(!hook_command("u", &curl).contains('\''), "a TOML literal string holds no quote");
        assert!(crate::pty::codex_guard_args("c", false, true).is_empty(), "not guarded");
        assert!(crate::pty::codex_guard_args("c", true, false).is_empty(), "Codex would drop it untrusted");
    }

    #[test]
    fn read_only_net_is_a_profile_and_never_a_sandbox_mode() {
        let args = crate::pty::codex_sandbox_args(READ_ONLY_NET);
        assert!(!args.iter().any(|a| a.starts_with("sandbox_mode")), "Codex's docs say not to set both");
        let profile = args[1].strip_prefix("default_permissions=\"").and_then(|p| p.strip_suffix('"')).expect("the profile is named first");
        assert!(args.contains(&format!("permissions.{profile}.extends=\":read-only\"")));
        assert!(args.contains(&format!("permissions.{profile}.network.enabled=true")));
        assert_eq!(args.iter().filter(|a| *a == "-c").count(), 3);
        assert_ne!(crate::pty::codex_sandbox_args(READ_ONLY_NET)[1], args[1], "no config can have rules ready for the next one's name");
        assert_eq!(crate::pty::codex_sandbox_args("read-only"), ["-c", "sandbox_mode=\"read-only\""]);
        assert_eq!(codex_windows_sandbox(READ_ONLY_NET), codex_windows_sandbox("read-only"), "it runs in the same sandbox");
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
