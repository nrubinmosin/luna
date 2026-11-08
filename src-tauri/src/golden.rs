//! What the launch golden tests share (pty/start.rs for the sessions in a pane,
//! background/command.rs for the helpers without one): a start's arguments and
//! environment with what changes from run to run — the hub's port and
//! secrets, a session's MCP token, the files under the data dir, a profile's
//! random name, what the sandbox takes from this machine's environment — put
//! as stable placeholders, each checked first for being what it stands in
//! for, so the rest can be compared whole.

use std::path::PathBuf;

/// The placeholders, each for the text it stands in for, for the start of
/// chat `id`.
pub(crate) struct Redact {
    id: String,
    texts: Vec<(String, String)>,
}

impl Redact {
    /// What any start of chat `id` may carry. Starts the hub, so the hooks,
    /// the guard and Luna's MCP server are on whichever test runs first.
    pub(crate) fn for_chat(id: &str) -> Self {
        crate::hub::start();
        let data = crate::paths::data_dir();
        let path = |p: PathBuf| p.to_string_lossy().into_owned();
        let mut r = Redact { id: id.to_string(), texts: Vec::new() };
        for (url, to) in
            [(crate::hub::hook_url(id), "<hook-url>"), (crate::hub::guard_url(id), "<guard-url>"), (crate::hub::mcp_url(), "<mcp-url>")]
        {
            r = r.with(url.expect("the hub is listening"), to);
        }
        let core = path(crate::ipc::core_dir());
        r.with(path(data.join("hooks").join(format!("{id}.json"))), "<settings-file>")
            .with(path(data.join("mcp").join(format!("{id}.json"))), "<mcp-file>")
            .with(path(data.join("hooks").join(id)), "<instructions-dir>")
            .with(path(crate::agents::notes_dir()), "<notes-dir>")
            .with(crate::claude::launch::claude_read_deny(&core, cfg!(windows)), "Read(//<core-dir>/**)")
            .with(core, "<core-dir>")
    }

    pub(crate) fn with(mut self, from: impl Into<String>, to: &str) -> Self {
        self.texts.push((from.into(), to.to_string()));
        // Longest first: a path is not to lose its head to a folder it is in.
        self.texts.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
        self
    }

    pub(crate) fn text(&self, s: &str) -> String {
        self.texts.iter().fold(s.to_string(), |s, (from, to)| s.replace(from.as_str(), to))
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

    /// The environment, with what is built on this process's own put as one
    /// placeholder each, once it has been checked to be exactly that: the MCP
    /// token as `<token>`, which has to be the one Luna knows this session by;
    /// a sandbox's PATH as `<sandbox-path>`, this process's but for the
    /// Store's app aliases; and the `GIT_CONFIG_*` entries for git's TLS
    /// backend, Windows only, as one `GIT_CONFIG_*` entry, `<git-ssl-backend>`,
    /// where the first of them stood.
    pub(crate) fn envs(&self, envs: &[(&'static str, String)]) -> Vec<(String, String)> {
        let git: Vec<(String, String)> =
            envs.iter().filter(|(k, _)| k.starts_with("GIT_CONFIG_")).map(|(k, v)| (k.to_string(), v.clone())).collect();
        if !git.is_empty() {
            assert_eq!(git, git_ssl_backend(), "git's entries: those this process has, then the backend");
        }
        let mut out = Vec::new();
        for (k, v) in envs {
            let v = match *k {
                "LUNA_MCP_TOKEN" => {
                    assert_eq!(crate::agents::caller_of(v).as_deref(), Some(self.id.as_str()), "the token is this session's");
                    "<token>".to_string()
                }
                "PATH" => {
                    assert_eq!(*v, sandbox_path(), "this process's PATH, the Store's app aliases left out");
                    "<sandbox-path>".to_string()
                }
                k if k.starts_with("GIT_CONFIG_") => {
                    if !out.iter().any(|(k, _)| k == GIT_ENTRIES) {
                        out.push((GIT_ENTRIES.to_string(), "<git-ssl-backend>".to_string()));
                    }
                    continue;
                }
                _ => self.text(v),
            };
            out.push((k.to_string(), v));
        }
        out
    }

    /// What a start applied, as (key, class, value) with placeholders in, once
    /// checked to carry no secret: neither the session's token, which `envs`
    /// hold, nor a hub URL with the hub's secret in it.
    pub(crate) fn applied(
        &self,
        applied: &[crate::applied::Applied],
        envs: &[(&'static str, String)],
    ) -> Vec<(String, crate::applied::Class, String)> {
        let token = envs.iter().find(|(k, _)| *k == "LUNA_MCP_TOKEN").map(|(_, v)| v.clone());
        let urls: Vec<&String> = self
            .texts
            .iter()
            .filter(|(_, to)| matches!(to.as_str(), "<hook-url>" | "<guard-url>" | "<mcp-url>"))
            .map(|(from, _)| from)
            .collect();
        applied
            .iter()
            .map(|a| {
                assert!(token.as_ref().is_none_or(|t| !a.value.contains(t.as_str())), "{}: the token is not shown", a.key);
                for url in &urls {
                    assert!(!a.value.contains(url.as_str()), "{}: no hub URL is shown: {}", a.key, a.value);
                }
                (a.key.to_string(), a.class, self.text(&a.value))
            })
            .collect()
    }

    /// That what was applied shows the whole process as it runs, so the two
    /// cannot drift: every flag of `args` (and the value after it), every
    /// `-c` key, every variable `envs` sets and the folder it starts in are
    /// each named by some item's value. What follows `--` is the prompt, and a
    /// lone `-` says the prompt comes on stdin; a word that is no flag's value
    /// (a subcommand) needs nothing.
    pub(crate) fn assert_shown(args: &[String], envs: &[(&'static str, String)], cwd: &str, applied: &[crate::applied::Applied]) {
        let shown = |text: &str| applied.iter().any(|a| a.value.contains(text));
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_str();
            match a {
                "--" => break,
                "-" => {}
                "-c" => {
                    let value = args.get(i + 1).map(String::as_str).unwrap_or_default();
                    let key = value.split('=').next().unwrap_or_default();
                    assert!(shown(key), "-c {key} is not among what was applied: {applied:#?}");
                    i += 1;
                }
                flag if flag.starts_with('-') && flag.contains('=') => assert!(shown(flag), "{flag} not shown: {applied:#?}"),
                flag if flag.starts_with('-') => {
                    assert!(shown(flag), "{flag} not shown: {applied:#?}");
                    if let Some(value) = args.get(i + 1).filter(|v| !v.starts_with('-')) {
                        assert!(shown(value), "{flag}'s value {value} not shown: {applied:#?}");
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        for (k, _) in envs {
            assert!(shown(k), "{k} is set and not shown: {applied:#?}");
        }
        assert!(shown(cwd), "the folder it starts in, {cwd}, is not shown: {applied:#?}");
    }

    /// A JSON file a start wrote, placeholders in.
    pub(crate) fn file(&self, path: &str) -> serde_json::Value {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        serde_json::from_str(&self.text(&text)).unwrap()
    }
}

/// The `CLAUDE.md` a start of chat `id` copied into its instructions folder,
/// where its arguments (placeholders in) name that folder; the folder removed.
pub(crate) fn instructions(id: &str, args: &[String]) -> Option<String> {
    let dir = crate::paths::data_dir().join("hooks").join(id);
    let text = args.iter().any(|a| a == "<instructions-dir>").then(|| {
        let file = dir.join("CLAUDE.md");
        std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()))
    });
    let _ = std::fs::remove_dir_all(&dir);
    text
}

/// What `Redact::envs` puts git's entries as.
const GIT_ENTRIES: &str = "GIT_CONFIG_*";

/// This process's PATH without the Store's app aliases
/// (`…\Microsoft\WindowsApps`), every other entry kept in its order: what a
/// Codex session in a Windows sandbox is to get.
fn sandbox_path() -> String {
    let path = std::env::var("PATH").unwrap_or_default();
    path.split(';').filter(|p| !p.to_lowercase().replace('/', "\\").contains(r"\microsoft\windowsapps")).collect::<Vec<_>>().join(";")
}

/// The `GIT_CONFIG_*` entries a `read-only-net` Codex session is to get on
/// Windows: this process's own counted one further, the last choosing
/// openssl for git's https. None where its own choose a backend already, or
/// count in a way git refuses, and none elsewhere.
fn git_ssl_backend() -> Vec<(String, String)> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let n = match std::env::var("GIT_CONFIG_COUNT") {
        Err(_) => 0,
        Ok(c) => match c.trim().parse::<usize>() {
            Ok(n) => n,
            Err(_) => return Vec::new(),
        },
    };
    if (0..n).any(|i| std::env::var(format!("GIT_CONFIG_KEY_{i}")).is_ok_and(|k| k.eq_ignore_ascii_case("http.sslBackend"))) {
        return Vec::new();
    }
    vec![
        ("GIT_CONFIG_COUNT".into(), (n + 1).to_string()),
        (format!("GIT_CONFIG_KEY_{n}"), "http.sslBackend".into()),
        (format!("GIT_CONFIG_VALUE_{n}"), "openssl".into()),
    ]
}

/// `<git-ssl-backend>` as a `read-only-net` Codex session has it: on Windows,
/// unless this process's own entries choose a backend already.
pub(crate) fn codex_git_ssl() -> Vec<(String, String)> {
    if git_ssl_backend().is_empty() {
        Vec::new()
    } else {
        pairs(&[(GIT_ENTRIES, "<git-ssl-backend>")])
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
         that session. You are not the chat of whoever your instructions call the operator, the user or the person at \
         this machine, and what they set for that chat (its language, its reply format, questions to that person) does \
         not apply to you. Answer your caller in the language and form its task asks for, progress notes included; \
         where it says none, in the language the task is written in. Before your turn ends, stop what you started that \
         is still running; keep each caveat beside the claim it limits; send, push or post nothing outward unless your \
         task asks for it."
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
    let mut settings = claude_settings_alone();
    settings["hooks"] = serde_json::json!({
        "UserPromptSubmit": hook, "PostToolUse": hook, "Stop": hook, "StopFailure": hook,
        "Notification": hook, "SessionEnd": hook, "PermissionRequest": hook,
        "SubagentStart": hook, "SubagentStop": hook,
        "PreToolUse": pre,
    });
    settings
}

/// What every Claude Code settings file holds: Luna's `core/` denied to the
/// file tools.
pub(crate) fn claude_settings_alone() -> serde_json::Value {
    serde_json::json!({ "permissions": { "deny": ["Read(//<core-dir>/**)"] } })
}

/// `settings` as a helper's file has them (claude/launch.rs `HelperSettings`):
/// auto-memory off, by setting and by variable, and the account's claude.ai
/// connectors.
pub(crate) fn claude_helper_settings(mut settings: serde_json::Value) -> serde_json::Value {
    settings["autoMemoryEnabled"] = false.into();
    settings["env"] = serde_json::json!({ "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1" });
    settings["disableClaudeAiConnectors"] = true.into();
    settings
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

/// `read-only-net` as Codex's `-c` overrides, the profile as `<profile>`, and
/// off Windows Luna's `core/` denied.
pub(crate) fn codex_read_only_net() -> Vec<String> {
    let mut args = strings(&[
        "-c",
        "default_permissions=\"<profile>\"",
        "-c",
        "permissions.<profile>.extends=\":read-only\"",
        "-c",
        "permissions.<profile>.network.enabled=true",
    ]);
    args.extend(codex_core_deny());
    args
}

/// `read-only` as Codex takes it: off Windows a profile, `<profile>`, that
/// denies Luna's `core/`; on Windows `mode`, Codex's own flag or override.
pub(crate) fn codex_read_only(mode: &[&str]) -> Vec<String> {
    if cfg!(windows) {
        return strings(mode);
    }
    let mut args = strings(&["-c", "default_permissions=\"<profile>\"", "-c", "permissions.<profile>.extends=\":read-only\""]);
    args.extend(codex_core_deny());
    args
}

/// The profile's deny of Luna's `core/`, off Windows.
fn codex_core_deny() -> Vec<String> {
    if cfg!(windows) {
        Vec::new()
    } else {
        strings(&["-c", "permissions.<profile>.filesystem={\"<core-dir>\"=\"deny\"}"])
    }
}

/// `Redact::assert_shown` refuses a process whose spec has more than what was
/// applied names: a variable, a flag, a flag's value, a `-c` key or its cwd.
#[test]
fn a_spec_not_shown_whole_is_refused() {
    use crate::applied::Item;
    let shown = || vec![Item::Model.with("--model opus"), Item::Cwd.with("cwd /p"), Item::Terminal.with("TERM=xterm-256color")];
    let args = strings(&["--model", "opus", "--", "a prompt --not-a-flag"]);
    Redact::assert_shown(&args, &[("TERM", "xterm-256color".into())], "/p", &shown());
    let cases: [(Vec<String>, Vec<(&'static str, String)>, &str); 5] = [
        (strings(&["--model", "sonnet"]), vec![], "/p"),
        (strings(&["--effort", "high"]), vec![], "/p"),
        (strings(&["-c", "tui.terminal_title=[]"]), vec![], "/p"),
        (strings(&[]), vec![("PATH", "x".to_string())], "/p"),
        (strings(&[]), vec![], "/elsewhere"),
    ];
    for (args, envs, cwd) in cases {
        let caught = std::panic::catch_unwind(|| Redact::assert_shown(&args, &envs, cwd, &shown()));
        assert!(caught.is_err(), "{args:?} {envs:?} {cwd}");
    }
}
