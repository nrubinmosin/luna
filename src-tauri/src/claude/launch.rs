//! How a Claude Code session in a pane is started: its arguments and
//! environment, laid out from what the start has already settled. Nothing
//! here writes, registers or mints anything; that happens under the chat's
//! spawn gate (pty/start.rs), which hands the results in.

use std::path::Path;

/// A Claude Code chat's start as `chat_args` lays it out: what the window
/// asked for, and what the start made for it under the spawn gate — the
/// files and the token, which this only names.
pub(crate) struct ChatStart<'a> {
    pub(crate) id: &'a str,
    pub(crate) account_path: &'a str,
    pub(crate) model: &'a str,
    pub(crate) effort: &'a str,
    pub(crate) permission_mode: &'a str,
    pub(crate) resume: Option<&'a str>,
    /// `--worktree`, by this name if it has one.
    pub(crate) worktree: Option<Option<&'a str>>,
    pub(crate) settings_file: Option<&'a Path>,
    pub(crate) agent: Option<&'a str>,
    pub(crate) read_only: bool,
    pub(crate) parent: Option<&'a str>,
    /// Luna's MCP config file and the token its header takes.
    pub(crate) mcp: Option<(&'a Path, &'a str)>,
    pub(crate) notes_dir: Option<&'a Path>,
    pub(crate) instructions_dir: Option<&'a Path>,
    pub(crate) skill_args: &'a [String],
    pub(crate) prompt: Option<&'a str>,
}

/// The arguments and environment of a Claude Code chat's start.
pub(crate) fn chat_args(s: &ChatStart) -> (Vec<String>, Vec<(&'static str, String)>) {
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let mut args = vec![
        "--model".to_string(),
        s.model.to_string(),
        "--effort".to_string(),
        s.effort.to_string(),
        "--permission-mode".to_string(),
        s.permission_mode.to_string(),
    ];
    if let Some(session_id) = s.resume {
        args.push("--resume".into());
        args.push(session_id.to_string());
    }
    if let Some(name) = s.worktree {
        args.push(crate::worktree::worktree_arg(name));
    }
    if let Some(settings) = s.settings_file {
        args.push("--settings".into());
        args.push(path(settings));
    }
    if let Some(agent) = s.agent {
        args.push("--agent".into());
        args.push(agent.to_string());
    }
    if s.read_only {
        args.push("--disallowedTools".into());
        args.push(CLAUDE_EDIT_TOOLS.join(","));
    }
    if let Some(parent) = s.parent {
        args.extend(claude_helper_args(parent));
    }
    if let Some((mcp, _)) = s.mcp {
        // Luna's tools without a permission prompt in every mode: the
        // user ticked the box that gives them. Variadic like
        // `--mcp-config`, so it goes where a flag follows it.
        args.push("--allowedTools".into());
        args.push("mcp__luna".into());
        args.push("--mcp-config".into());
        args.push(path(mcp));
    }
    // A long prompt or message from its agent comes as a file there
    // (agents.rs `spill`), and so does a helper's answer named in a
    // notice, to read without asking.
    if let Some(notes) = s.notes_dir {
        args.push("--add-dir".into());
        args.push(path(notes));
    }
    if let Some(dir) = s.instructions_dir {
        args.push("--add-dir".into());
        args.push(path(dir));
    }
    args.extend(s.skill_args.iter().cloned());
    if let Some(p) = s.prompt.map(str::trim).filter(|p| !p.is_empty()) {
        // `--mcp-config` takes any number of files, and took the prompt
        // after it for one more: "MCP config file not found: <prompt>".
        args.push("--".into());
        args.push(p.to_string());
    }
    let mut envs = vec![
        ("CLAUDE_CONFIG_DIR", s.account_path.to_string()),
        // Luna updates the CLI itself; the CLI's own updater would install a
        // second copy under the user profile that nothing here ever runs.
        ("DISABLE_AUTOUPDATER", "1".to_string()),
    ];
    if s.instructions_dir.is_some() {
        envs.push((CLAUDE_ADDED_DIR_MEMORY, "1".to_string()));
    }
    if let Some((_, token)) = s.mcp {
        envs.push((CLAUDE_MCP_TOKEN_ENV, token.to_string()));
    }
    envs.extend(crate::pty::session_envs(s.id, s.parent));
    (args, envs)
}

/// What `claude_instructions_dir` asks of a session's start: the folder and
/// the variable that has its `CLAUDE.md` read.
pub(crate) const CLAUDE_ADDED_DIR_MEMORY: &str = "CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD";

/// A hook command posting its stdin to `url`. curl.exe by name on Windows:
/// there `curl` in PowerShell is an alias for Invoke-WebRequest. Its stdin —
/// the hook's JSON — goes up as the body; a 204 answer has no body, so nothing
/// comes back to print, and the guard's refusal is printed for the CLI to read.
fn hook_command(url: &str) -> String {
    let curl = if cfg!(windows) { "curl.exe" } else { "curl" };
    format!("{curl} -s -m 3 -X POST --data-binary @- {url}")
}

/// `claude_settings_file`'s settings: the session's hooks to `hook_url`, the
/// guard's to `guard_url`, and the skills and instructions it does not get.
pub(crate) fn claude_settings(
    hook_url: Option<&str>,
    guard_url: Option<&str>,
    library: &crate::library::SessionSkills,
) -> serde_json::Map<String, serde_json::Value> {
    let mut settings = serde_json::Map::new();
    let mut hooks = serde_json::Map::new();
    let mut pre_tool_use = Vec::new();
    if let Some(url) = hook_url {
        let command = hook_command(url);
        let hook = |timeout: u32| serde_json::json!([{ "hooks": [{ "type": "command", "command": command, "timeout": timeout }] }]);
        for event in ["UserPromptSubmit", "PostToolUse", "Stop", "StopFailure", "Notification", "SessionEnd", "PermissionRequest"] {
            hooks.insert(event.into(), hook(5));
        }
        // A subagent's life, which a background agent lives past the turn
        // that started it (activity.rs `HookEvent::AgentAtWork`).
        hooks.insert("SubagentStart".into(), hook(5));
        hooks.insert("SubagentStop".into(), hook(5));
        // What a prompt asks, which neither the screen nor the transcript
        // gives whole: the question and its options (the transcript gets the
        // call only once it is answered), the command up for leave.
        pre_tool_use
            .push(serde_json::json!({ "matcher": "AskUserQuestion", "hooks": [{ "type": "command", "command": command, "timeout": 5 }] }));
    }
    if let Some(url) = guard_url {
        pre_tool_use.push(serde_json::json!({ "matcher": crate::guard::TOOLS, "hooks": [{ "type": "command", "command": hook_command(url), "timeout": 5 }] }));
    }
    if !pre_tool_use.is_empty() {
        hooks.insert("PreToolUse".into(), pre_tool_use.into());
    }
    if !hooks.is_empty() {
        settings.insert("hooks".into(), hooks.into());
    }
    if !library.hidden.is_empty() {
        settings.insert("skillOverrides".into(), library.hidden.iter().map(|n| (n.clone(), serde_json::json!("off"))).collect());
    }
    if !library.excluded.is_empty() {
        settings.insert("claudeMdExcludes".into(), library.excluded.clone().into());
    }
    settings
}

/// `helper_line` for Claude Code, added to its system prompt.
pub(crate) fn claude_helper_args(parent: &str) -> Vec<String> {
    vec!["--append-system-prompt".into(), crate::pty::helper_line(parent)]
}

/// What `readOnly` takes away from Claude Code. Bash stays, and can still
/// write: it is a checker's tool, and no flag tells its reads from its writes.
pub(crate) const CLAUDE_EDIT_TOOLS: [&str; 3] = ["Edit", "Write", "NotebookEdit"];

/// `readOnly` and `agent` as they show in a Claude Code session's settings,
/// where they are set: a helper opened as a chat takes them from there.
pub(crate) fn with_role(mut settings: serde_json::Value, read_only: bool, agent: Option<&str>) -> serde_json::Value {
    if read_only {
        settings["readOnly"] = true.into();
    }
    if let Some(agent) = agent {
        settings["agent"] = agent.into();
    }
    settings
}

/// What a Claude Code session's MCP header takes its bearer token from, as
/// Codex's `bearer_token_env_var` does: set in the session's environment
/// (`CLAUDE_MCP_TOKEN_ENV`), never written to the config file, where any
/// other session of the user's could read it and act as this one.
pub(crate) const CLAUDE_MCP_TOKEN_ENV: &str = "LUNA_MCP_TOKEN";
