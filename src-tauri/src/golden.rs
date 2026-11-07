//! What the launch golden tests share (pty/start.rs for the sessions in a pane,
//! background.rs for the helpers without one): a start's arguments and
//! environment with what changes from run to run — the hub's port and
//! secrets, a session's MCP token, the files under the data dir, a profile's
//! random name — put as stable placeholders, so the rest can be compared
//! whole.

use std::path::PathBuf;

/// The placeholders, each for the text it stands in for.
pub(crate) struct Redact(Vec<(String, String)>);

impl Redact {
    /// What any start of chat `id` may carry. Starts the hub, so the hooks,
    /// the guard and Luna's MCP server are on whichever test runs first.
    pub(crate) fn for_chat(id: &str) -> Self {
        crate::hub::start();
        let data = crate::paths::data_dir();
        let path = |p: PathBuf| p.to_string_lossy().into_owned();
        let mut r = Redact(Vec::new());
        for (url, to) in
            [(crate::hub::hook_url(id), "<hook-url>"), (crate::hub::guard_url(id), "<guard-url>"), (crate::hub::mcp_url(), "<mcp-url>")]
        {
            r = r.with(url.expect("the hub is listening"), to);
        }
        r.with(path(data.join("hooks").join(format!("{id}.json"))), "<settings-file>")
            .with(path(data.join("mcp").join(format!("{id}.json"))), "<mcp-file>")
            .with(path(data.join("hooks").join(id)), "<instructions-dir>")
            .with(path(crate::agents::notes_dir()), "<notes-dir>")
    }

    pub(crate) fn with(mut self, from: impl Into<String>, to: &str) -> Self {
        self.0.push((from.into(), to.to_string()));
        // Longest first: a path is not to lose its head to a folder it is in.
        self.0.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
        self
    }

    pub(crate) fn text(&self, s: &str) -> String {
        self.0.iter().fold(s.to_string(), |s, (from, to)| s.replace(from.as_str(), to))
    }

    /// The arguments, a `read-only-net` profile's new name on every start
    /// put as `<profile>`.
    pub(crate) fn args(&self, args: &[String]) -> Vec<String> {
        let profile = args.iter().find_map(|a| a.strip_prefix("default_permissions=\"")?.strip_suffix('"').map(str::to_owned));
        let args: Vec<String> = args.iter().map(|a| self.text(a)).collect();
        match profile {
            Some(p) => args.into_iter().map(|a| a.replace(&p, "<profile>")).collect(),
            None => args,
        }
    }

    /// The environment, the MCP token as `<token>` and a sandbox's PATH as
    /// `<sandbox-path>` (checked for what it leaves out). `GIT_CONFIG_*` is
    /// built on the host's own (codex/launch.rs `git_ssl_backend_envs`), so it is left
    /// out, and only Windows may have it.
    pub(crate) fn envs(&self, envs: &[(&'static str, String)]) -> Vec<(String, String)> {
        assert!(cfg!(windows) || !envs.iter().any(|(k, _)| k.starts_with("GIT_CONFIG_")), "{envs:?}");
        envs.iter()
            .filter(|(k, _)| !k.starts_with("GIT_CONFIG_"))
            .map(|(k, v)| {
                let v = match *k {
                    "LUNA_MCP_TOKEN" => {
                        assert!(!v.is_empty(), "a token");
                        "<token>".to_string()
                    }
                    "PATH" => {
                        assert!(!v.to_lowercase().contains("windowsapps"), "{v}");
                        "<sandbox-path>".to_string()
                    }
                    _ => self.text(v),
                };
                (k.to_string(), v)
            })
            .collect()
    }

    /// A JSON file a start wrote, placeholders in.
    pub(crate) fn file(&self, path: &str) -> serde_json::Value {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        serde_json::from_str(&self.text(&text)).unwrap()
    }
}

/// `args` as owned strings, for the expected side of a comparison.
pub(crate) fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

/// `envs` as owned pairs, likewise.
pub(crate) fn pairs(envs: &[(&str, &str)]) -> Vec<(String, String)> {
    envs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// The curl a Claude Code hook runs, by name.
pub(crate) fn claude_curl() -> &'static str {
    if cfg!(windows) {
        "curl.exe"
    } else {
        "curl"
    }
}

/// The curl a Codex hook runs: by its full path where Windows keeps it.
pub(crate) fn codex_curl() -> String {
    if cfg!(windows) {
        if let Ok(root) = std::env::var("SystemRoot") {
            let full = format!(r"{root}\System32\curl.exe");
            if !root.contains([' ', '\'', '"']) && std::path::Path::new(&full).is_file() {
                return full;
            }
        }
        return "curl.exe".into();
    }
    "curl".into()
}

/// What a helper started by `parent` is told, word for word.
pub(crate) fn helper_line(parent: &str) -> String {
    format!(
        "You are a helper session started through Luna by session {parent}, not a chat with a person: your answers go to \
         that session. Instructions meant only for the session a person talks to do not apply to you. Write in the \
         language of the task you were given, progress notes included."
    )
}

/// The settings file's hooks for a chat (`<hook-url>`), with the guard's
/// PreToolUse entry (`<guard-url>`) after the question's when `guard`.
pub(crate) fn claude_chat_settings(guard: bool) -> serde_json::Value {
    let command = format!("{} -s -m 3 -X POST --data-binary @- <hook-url>", claude_curl());
    let hook = serde_json::json!([{ "hooks": [{ "type": "command", "command": command, "timeout": 5 }] }]);
    let mut pre =
        vec![serde_json::json!({ "matcher": "AskUserQuestion", "hooks": [{ "type": "command", "command": command, "timeout": 5 }] })];
    if guard {
        pre.push(claude_guard_entry());
    }
    serde_json::json!({
        "hooks": {
            "UserPromptSubmit": hook, "PostToolUse": hook, "Stop": hook, "StopFailure": hook,
            "Notification": hook, "SessionEnd": hook, "PermissionRequest": hook,
            "SubagentStart": hook, "SubagentStop": hook,
            "PreToolUse": pre,
        }
    })
}

/// The guard's PreToolUse entry in a Claude Code settings file.
pub(crate) fn claude_guard_entry() -> serde_json::Value {
    let command = format!("{} -s -m 3 -X POST --data-binary @- <guard-url>", claude_curl());
    serde_json::json!({ "matcher": "Bash|PowerShell|Monitor", "hooks": [{ "type": "command", "command": command, "timeout": 5 }] })
}

/// The MCP config a tooled Claude Code session is given.
pub(crate) fn claude_mcp_config() -> serde_json::Value {
    serde_json::json!({
        "mcpServers": { "luna": {
            "type": "http", "url": "<mcp-url>", "headers": { "Authorization": "Bearer ${LUNA_MCP_TOKEN}" },
            "timeout": 1_900_000, "alwaysLoad": true,
        } }
    })
}

/// The guard as Codex's `-c` value.
pub(crate) fn codex_guard() -> String {
    format!(
        r#"hooks.PreToolUse=[{{matcher="Bash",hooks=[{{type="command",command='{} -s -m 3 -X POST --data-binary "@-" <guard-url>',timeout=5}}]}}]"#,
        codex_curl()
    )
}

/// Luna's MCP server as Codex's `-c` overrides.
pub(crate) fn codex_mcp() -> Vec<String> {
    strings(&[
        "-c",
        "mcp_servers.luna.url=\"<mcp-url>\"",
        "-c",
        "mcp_servers.luna.bearer_token_env_var=\"LUNA_MCP_TOKEN\"",
        "-c",
        "mcp_servers.luna.tool_timeout_sec=1900",
        "-c",
        "mcp_servers.luna.default_tools_approval_mode=\"approve\"",
    ])
}

/// Codex's Windows sandbox override, where a mode has one.
pub(crate) fn codex_windows_sandbox() -> Vec<String> {
    if cfg!(windows) {
        strings(&["-c", "windows.sandbox=\"unelevated\""])
    } else {
        Vec::new()
    }
}

/// The PATH entry a sandboxed Codex session gets, Windows only.
pub(crate) fn codex_sandbox_path() -> Vec<(String, String)> {
    if cfg!(windows) {
        pairs(&[("PATH", "<sandbox-path>")])
    } else {
        Vec::new()
    }
}

/// `read-only-net` as Codex's `-c` overrides, the profile as `<profile>`.
pub(crate) fn codex_read_only_net() -> Vec<String> {
    strings(&[
        "-c",
        "default_permissions=\"<profile>\"",
        "-c",
        "permissions.<profile>.extends=\":read-only\"",
        "-c",
        "permissions.<profile>.network.enabled=true",
    ])
}
