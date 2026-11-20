//! Luna's status hooks for a Codex chat (activity/hooks.rs takes what they
//! post): each event Luna listens to, given at launch as a `-c hooks.<Event>`
//! override, and trusted one by one by a `-c hooks.state` entry of Luna's own
//! with the hash Codex itself computes for that hook. Nothing is written into
//! the account: its own `hooks.state`, which Codex reads from the user layer
//! beside the session flags', stays as it is, and so does every other hook's
//! review. Pure: the URL travels in the environment (`HOOK_URL_ENV`), so the
//! hooks and their hashes are the same for every chat.

use sha2::Digest;

/// The variable that carries the chat's hook URL, with its secret, into the
/// hooks' environment: never on a command line, which other users may read off
/// Windows (`ps`, `/proc/<pid>/cmdline`). Codex runs a hook with its own
/// environment (0.169.0 `hooks/src/registry.rs`).
pub(crate) const HOOK_URL_ENV: &str = "LUNA_HOOK_URL";

/// Each event Luna listens to, Codex's name for it in `-c hooks.<Event>`, the
/// label its trust key and hash use (`hook_event_key_label`, 0.169.0
/// `hooks/src/lib.rs`), and its timeout in seconds: `Interrupt` and
/// `SessionEnd` are held to at most 3 (`normalize_command_hook`), and the hash
/// is of the timeout as Codex holds it.
pub(crate) const EVENTS: [(&str, &str, u64); 8] = [
    ("UserPromptSubmit", "user_prompt_submit", 5),
    ("PostToolUse", "post_tool_use", 5),
    ("PermissionRequest", "permission_request", 5),
    ("Stop", "stop", 5),
    ("SubagentStart", "subagent_start", 5),
    ("SubagentStop", "subagent_stop", 5),
    ("Interrupt", "interrupt", 3),
    ("SessionEnd", "session_end", 3),
];

/// The hook's command: its stdin, the event's JSON, posted to the URL in
/// `HOOK_URL_ENV`. Codex runs it in the session's own shell (pwsh or Windows
/// PowerShell on Windows, cmd where it has neither, sh, bash or zsh elsewhere:
/// 0.169.0 `build_hooks_config`), so nothing here is any shell's syntax: the
/// variable is read by curl itself (`--variable`, curl 8.3 and later), `"@-"`
/// is quoted for PowerShell's splatting, and `curl.exe` by name on Windows,
/// where `curl` in Windows PowerShell is Invoke-WebRequest. A curl too old
/// fails the hook, and the record reads the rollout alone.
pub(crate) fn command(windows: bool) -> String {
    let curl = if windows { "curl.exe" } else { "curl" };
    format!("{curl} -s -m 2 -X POST --data-binary \"@-\" --variable \"%{HOOK_URL_ENV}\" --expand-url \"{{{{{HOOK_URL_ENV}}}}}\"")
}

/// Where Codex says a `-c` override comes from, as its hooks' trust keys name
/// it (`synthetic_layer_path("<session-flags>/config.toml")`, 0.169.0
/// `hooks/src/engine/discovery.rs`).
fn session_flags_source(windows: bool) -> &'static str {
    if windows {
        r"C:\<session-flags>\config.toml"
    } else {
        "/<session-flags>/config.toml"
    }
}

/// The trust key of the one hook Luna gives `label`: its source, the event,
/// and its place, the first handler of the first group (`hook_key`).
pub(crate) fn trust_key(label: &str, windows: bool) -> String {
    format!("{}:{label}:0:0", session_flags_source(windows))
}

/// The hash Codex trusts a hook by (`hook_hash`, 0.169.0): SHA-256 of the
/// hook's normalized identity as JSON, keys sorted, no spaces — the event's
/// label, and the one handler as Codex holds it (its `async` false, no matcher,
/// no status message: what is absent is left out).
pub(crate) fn trusted_hash(label: &str, command: &str, timeout: u64) -> String {
    // Keys in sorted order, as Codex sorts them before it hashes.
    let identity = serde_json::json!({
        "event_name": label,
        "hooks": [{ "async": false, "command": command, "timeout": timeout, "type": "command" }],
    });
    let digest = sha2::Sha256::digest(identity.to_string().as_bytes());
    format!("sha256:{}", digest.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// The `-c` overrides that give a chat Luna's hooks and trust each of them:
/// one `hooks.<Event>` per event, then one `hooks.state` table naming them
/// all, as a later `-c` of the same key would replace the whole table.
pub(crate) fn status_hook_args(windows: bool) -> Vec<String> {
    let command = command(windows);
    let mut args = Vec::new();
    let mut state = toml_edit::InlineTable::new();
    for (event, label, timeout) in EVENTS {
        let mut handler = toml_edit::InlineTable::new();
        handler.insert("type", "command".into());
        handler.insert("command", command.as_str().into());
        handler.insert("timeout", (timeout as i64).into());
        let mut group = toml_edit::InlineTable::new();
        group.insert("hooks", toml_edit::Value::Array([toml_edit::Value::InlineTable(handler)].into_iter().collect()));
        let groups: toml_edit::Array = [toml_edit::Value::InlineTable(group)].into_iter().collect();
        args.extend(["-c".to_string(), format!("hooks.{event}={groups}")]);
        let mut trust = toml_edit::InlineTable::new();
        trust.insert("trusted_hash", trusted_hash(label, &command, timeout).into());
        state.insert(trust_key(label, windows), toml_edit::Value::InlineTable(trust));
    }
    args.extend(["-c".to_string(), format!("hooks.state={state}")]);
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hashes Codex 0.169.0 itself computed for these hooks on Windows
    /// (`hooks/list` over `codex app-server`, 2026-11-20: each `trusted`, its
    /// `currentHash` this): a Codex that hashes otherwise fails this first.
    #[test]
    fn the_hashes_are_the_ones_codex_computes() {
        let command = command(true);
        let seen = [
            ("user_prompt_submit", 5, "sha256:2dbe7b4be5264d2a79cb8f68bde759985c1f74a033bbffbb03542cccd3c7b5b3"),
            ("post_tool_use", 5, "sha256:30fe79217b7f7150cf7f8360f211dda28389ba14709fe6c9c64283fc94be791c"),
            ("permission_request", 5, "sha256:86402154e1be3e2a3136eadf142b0538e9d0cf4e438fbe6a35c8c4b7b5b9b52c"),
            ("stop", 5, "sha256:e76aeccde16a2876e42152fff66664da34f9d710718096490474fa9ee6855657"),
            ("subagent_start", 5, "sha256:e53c4fd276a0f8fdd064f8b4ec1b81770153f30af383f6d31edd71c8b1cf6bb3"),
            ("subagent_stop", 5, "sha256:851017ba9acf5155cc6dcd5289651a576abb9ab8631a06ffabfc4a3c8ec63440"),
            ("interrupt", 3, "sha256:59a4829ab764d57eb9442a1ad34565bfd45ed04962150e61460ef688270fd16e"),
            ("session_end", 3, "sha256:2c9686f676a270d8a27966a0ff7fca4a2c20bd1e8ecaec991ea540bec487ea41"),
        ];
        for (label, timeout, hash) in seen {
            assert_eq!(trusted_hash(label, &command, timeout), hash, "{label}");
            assert!(EVENTS.iter().any(|e| e.1 == label && e.2 == timeout), "{label}");
        }
        assert_eq!(trust_key("stop", true), r"C:\<session-flags>\config.toml:stop:0:0");
    }

    /// What the overrides say, read back as Codex parses a `-c` value: each
    /// event's one command, and one trust entry for each by its key.
    #[test]
    fn every_hook_is_given_and_trusted_by_one_table() {
        for windows in [true, false] {
            let args = status_hook_args(windows);
            assert_eq!(args.len(), 2 * (EVENTS.len() + 1));
            assert!(args.iter().step_by(2).all(|a| a == "-c"));
            let values: Vec<&String> = args.iter().skip(1).step_by(2).collect();
            for (i, (event, _, timeout)) in EVENTS.iter().enumerate() {
                let (key, value) = values[i].split_once('=').unwrap();
                assert_eq!(key, format!("hooks.{event}"));
                let doc: toml_edit::DocumentMut = format!("v = {value}").parse().unwrap();
                let handler = &doc["v"][0]["hooks"][0];
                assert_eq!(handler["type"].as_str(), Some("command"));
                assert_eq!(handler["command"].as_str(), Some(command(windows).as_str()));
                assert_eq!(handler["timeout"].as_integer(), Some(*timeout as i64));
            }
            let (key, value) = values[EVENTS.len()].split_once('=').unwrap();
            assert_eq!(key, "hooks.state", "a dotted key would split the trust key at its dots");
            let doc: toml_edit::DocumentMut = format!("v = {value}").parse().unwrap();
            let state = doc["v"].as_inline_table().unwrap();
            assert_eq!(state.len(), EVENTS.len());
            for (_, label, timeout) in EVENTS {
                let entry = state.get(&trust_key(label, windows)).and_then(|v| v.as_inline_table()).expect(label);
                assert_eq!(
                    entry.get("trusted_hash").and_then(|v| v.as_str()),
                    Some(trusted_hash(label, &command(windows), timeout).as_str())
                );
            }
        }
    }

    /// The URL is never in the command, so its secret is in no command line.
    #[test]
    fn the_command_names_the_variable_not_the_url() {
        for windows in [true, false] {
            let c = command(windows);
            assert!(c.contains("--variable \"%LUNA_HOOK_URL\"") && c.contains("--expand-url \"{{LUNA_HOOK_URL}}\""), "{c}");
            assert!(!c.contains("http"), "{c}");
        }
    }
}
