//! Luna's status hooks for a Codex chat (activity/hooks.rs takes what they
//! post): each event Luna listens to, given at launch as a `-c hooks.<Event>`
//! override, and trusted one by one by a `-c hooks.state` entry of Luna's own
//! with the hash Codex itself computes for that hook. Nothing is written into
//! the account: its own `hooks.state`, which Codex reads from the user layer
//! beside the session flags', stays as it is, and so does every other hook's
//! review. Pure: the URL travels in the environment (`HOOK_URL_ENV`), so the
//! hooks and their hashes are the same for every chat on one machine, and the
//! curl they run is resolved at launch by the caller.

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
/// `HOOK_URL_ENV`, by the curl at `curl`, an absolute path resolved at launch
/// (pty/start.rs `codex_hook_curl`), so no shell looks for one in the
/// project's folder (cmd does, before PATH). Codex runs it in the session's
/// own shell (pwsh or Windows PowerShell on Windows, cmd where it has neither,
/// sh, bash or zsh elsewhere: 0.169.0 `build_hooks_config`), so nothing here is
/// any shell's syntax: the path is bare (`usable_curl` takes none a shell
/// would read otherwise), the variable is read by curl itself (`--variable`,
/// curl 8.3 and later), `"@-"` and `"*"` are quoted for PowerShell. And it
/// stays on loopback and quiet: `-q` first, so no `.curlrc` or `_curlrc` adds a
/// proxy or a trace; `--noproxy "*"`, so no proxy variable the session
/// inherits sends the URL and its secret, or the hook's body, anywhere; the
/// hub's empty answer to the null device and `-s`, so nothing comes back for
/// Codex to read as the hook's output. A curl too old fails the hook, and the
/// record reads the rollout alone.
pub(crate) fn command(curl: &str, windows: bool) -> String {
    let null = if windows { "NUL" } else { "/dev/null" };
    format!(
        "{curl} -q -s -o {null} --noproxy \"*\" -m 2 -X POST --data-binary \"@-\" --variable \"%{HOOK_URL_ENV}\" --expand-url \"{{{{{HOOK_URL_ENV}}}}}\""
    )
}

/// Whether a curl path can stand bare at the head of the command in every
/// shell Codex may run it in: absolute, and of letters, digits and `\ / : . _
/// -` only, so no space splits it and no shell expands anything in it. One
/// that cannot gives the chat no hooks.
pub(crate) fn usable_curl(path: &str, windows: bool) -> bool {
    let absolute = if windows { path.get(1..3) == Some(":\\") } else { path.starts_with('/') };
    absolute && path.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '\\' | '/' | ':' | '.' | '_' | '-'))
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
pub(crate) fn status_hook_args(curl: &str, windows: bool) -> Vec<String> {
    let command = command(curl, windows);
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

    /// The curl Windows has, as the clone's checks resolved it.
    const WINDOWS_CURL: &str = r"C:\WINDOWS\System32\curl.exe";
    const UNIX_CURL: &str = "/usr/bin/curl";

    /// The hashes Codex 0.169.0 itself computed for these hooks on Windows,
    /// the curl at `WINDOWS_CURL` (`hooks/list` over `codex app-server`,
    /// 2026-11-20: each `trusted`, its `currentHash` this; without Luna's
    /// entries each `untrusted`). Fixed strings: a Codex that hashes otherwise
    /// is caught live (docs/cli-compatibility.md), not here.
    #[test]
    fn the_hashes_are_the_ones_codex_computes() {
        let command = command(WINDOWS_CURL, true);
        let seen = [
            ("user_prompt_submit", 5, "sha256:37d25fb513a0f90f987753108089e86d17e188b71d6273e156bdb09da3509a42"),
            ("post_tool_use", 5, "sha256:2712a8670c86bb43eb8e18c6f1024559ccf0c3ab8576feab4557cdbde21db73b"),
            ("permission_request", 5, "sha256:74b436a3aa928b79b2350f5ed5a17856e16897b60468d0ed12500b25c311dc10"),
            ("stop", 5, "sha256:456fe622872ba863f8a019e525f0dc882692f8c05e916614c0df4fad2a01ed90"),
            ("subagent_start", 5, "sha256:a3845432fe47bb02e5dde64fc697667d9a78b8e1278e6a6f357aa952badbd223"),
            ("subagent_stop", 5, "sha256:ffa1e6696d01e490328bade03b8de713c816d28418c736ad0698062e00ff90b5"),
            ("interrupt", 3, "sha256:c57f943086f7da7f657b3994da24f634ad09b987ec0054b43ff05d7f82f83115"),
            ("session_end", 3, "sha256:d3bfffb0ddb198fff0e9e77e7fa093d638519c655f0f49341b00ecd172b3f8ae"),
        ];
        for (label, timeout, hash) in seen {
            assert_eq!(trusted_hash(label, &command, timeout), hash, "{label}");
            assert!(EVENTS.iter().any(|e| e.1 == label && e.2 == timeout), "{label}");
        }
        assert_eq!(trust_key("stop", true), r"C:\<session-flags>\config.toml:stop:0:0");
    }

    /// Off Windows the key's source is `synthetic_layer_path` resolved against
    /// `/` (0.169.0 `hooks/src/engine/discovery.rs`); read in the source, not
    /// seen live. A wrong one shows Codex's review dialog in every chat there.
    #[test]
    fn the_unix_trust_key_is_the_session_flags_path_from_root() {
        assert_eq!(trust_key("user_prompt_submit", false), "/<session-flags>/config.toml:user_prompt_submit:0:0");
        for (_, label, _) in EVENTS {
            let key = trust_key(label, false);
            assert!(key.starts_with("/<session-flags>/config.toml:") && key.ends_with(":0:0") && !key.contains('\\'), "{key}");
        }
    }

    /// What the overrides say, read back as Codex parses a `-c` value: each
    /// event's one command, and one trust entry for each by its key.
    #[test]
    fn every_hook_is_given_and_trusted_by_one_table() {
        for (curl, windows) in [(WINDOWS_CURL, true), (UNIX_CURL, false)] {
            let args = status_hook_args(curl, windows);
            assert_eq!(args.len(), 2 * (EVENTS.len() + 1));
            assert!(args.iter().step_by(2).all(|a| a == "-c"));
            let values: Vec<&String> = args.iter().skip(1).step_by(2).collect();
            for (i, (event, _, timeout)) in EVENTS.iter().enumerate() {
                let (key, value) = values[i].split_once('=').unwrap();
                assert_eq!(key, format!("hooks.{event}"));
                let doc: toml_edit::DocumentMut = format!("v = {value}").parse().unwrap();
                let handler = &doc["v"][0]["hooks"][0];
                assert_eq!(handler["type"].as_str(), Some("command"));
                assert_eq!(handler["command"].as_str(), Some(command(curl, windows).as_str()));
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
                    Some(trusted_hash(label, &command(curl, windows), timeout).as_str())
                );
            }
        }
    }

    /// The command, word for word: curl by its absolute path, no curlrc, no
    /// proxy, nothing printed, and the URL (with its secret) only by the
    /// variable's name.
    #[test]
    fn the_command_stays_on_loopback_quiet_and_names_the_variable_not_the_url() {
        assert_eq!(
            command(WINDOWS_CURL, true),
            r#"C:\WINDOWS\System32\curl.exe -q -s -o NUL --noproxy "*" -m 2 -X POST --data-binary "@-" --variable "%LUNA_HOOK_URL" --expand-url "{{LUNA_HOOK_URL}}""#
        );
        assert_eq!(
            command(UNIX_CURL, false),
            r#"/usr/bin/curl -q -s -o /dev/null --noproxy "*" -m 2 -X POST --data-binary "@-" --variable "%LUNA_HOOK_URL" --expand-url "{{LUNA_HOOK_URL}}""#
        );
        for (curl, windows) in [(WINDOWS_CURL, true), (UNIX_CURL, false)] {
            let c = command(curl, windows);
            assert!(c.starts_with(&format!("{curl} -q ")), "-q is read only as the first argument: {c}");
            assert!(!c.contains("http"), "{c}");
        }
    }

    /// Only a path every shell takes bare: absolute, nothing a shell splits on
    /// or expands.
    #[test]
    fn only_a_bare_absolute_curl_is_used() {
        assert!(usable_curl(r"C:\WINDOWS\System32\curl.exe", true));
        assert!(usable_curl("/usr/bin/curl", false));
        assert!(usable_curl("/opt/homebrew/opt/curl-8.9_1/bin/curl", false));
        for (path, windows) in [
            ("curl.exe", true),
            (r"System32\curl.exe", true),
            (r"C:\Program Files\curl\curl.exe", true),
            (r"C:\x&y\curl.exe", true),
            (r"C:\%P%\curl.exe", true),
            ("curl", false),
            ("/home/a b/curl", false),
            ("/home/$USER/curl", false),
            ("/opt/`x`/curl", false),
            ("/opt/x'y/curl", false),
        ] {
            assert!(!usable_curl(path, windows), "{path}");
        }
    }
}
