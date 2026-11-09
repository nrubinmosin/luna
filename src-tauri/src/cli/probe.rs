//! The offline probe: what a staged CLI must show before it may become
//! current. It runs the new binary a few times with nothing of an account
//! (a scratch home of its own, no network asked for, no prompt):
//!
//! - `--version` names the version that was fetched;
//! - `--help` of each command Luna launches still lists every flag Luna
//!   passes it (`COMMANDS`, which the golden launch tests hold to every
//!   argument list they pin: `covers`);
//! - Codex takes the hooks and MCP overrides as Luna writes them: it loads
//!   its configuration with them (`features list`) without refusing it.
//!
//! A flag gone from the help is the commonest way a CLI update breaks the
//! sessions Luna starts, and the cheapest to see before anyone's chat does.
//! What only a real turn shows is the canary's (`canary`).

use crate::provider::Provider;
use std::path::Path;
use std::process::{Command as Process, Stdio};
use std::time::{Duration, Instant};

/// One command Luna launches, as the words after the exe, and the flags it
/// passes there.
pub(crate) struct Command {
    pub(crate) path: &'static [&'static str],
    pub(crate) flags: &'static [&'static str],
}

/// Claude Code: chats and helpers alike are the bare command
/// (claude/launch.rs, pty/start.rs, background/command.rs). 2.1.318 lists
/// `--permission-prompt-tool` only in another flag's text, which the probe
/// takes as listed (`listed`).
const CLAUDE: &[Command] = &[Command {
    path: &[],
    flags: &[
        "--model",
        "--effort",
        "--permission-mode",
        "--settings",
        "--resume",
        "--worktree",
        "--mcp-config",
        "--strict-mcp-config",
        "--allowedTools",
        "--disallowedTools",
        "--append-system-prompt",
        "--agent",
        "--add-dir",
        "--permission-prompt-tool",
        "--dangerously-skip-permissions",
        "-p",
        "--input-format",
        "--output-format",
        "--verbose",
        "--replay-user-messages",
    ],
}];

/// Codex: a chat (codex/launch.rs), its resume, a helper's `exec` and its
/// `exec resume` (background/command.rs), `login`, and the model list
/// (codex/models.rs). `exec resume` takes fewer than `exec`: no folder,
/// worktree or sandbox flag.
const CODEX: &[Command] = &[
    Command {
        path: &[],
        flags: &[
            "-C",
            "--no-daemon",
            "--worktree",
            "--model",
            "-c",
            "--ask-for-approval",
            "--sandbox",
            "--approve-for-me",
            "--dangerously-bypass-approvals-and-sandbox",
            "--dangerously-bypass-hook-trust",
        ],
    },
    Command {
        path: &["resume"],
        flags: &[
            "-C",
            "--no-daemon",
            "--model",
            "-c",
            "--ask-for-approval",
            "--sandbox",
            "--approve-for-me",
            "--dangerously-bypass-approvals-and-sandbox",
            "--dangerously-bypass-hook-trust",
        ],
    },
    Command {
        path: &["exec"],
        flags: &[
            "-C",
            "--worktree",
            "--json",
            "--skip-git-repo-check",
            "--model",
            "-c",
            "--approve-for-me",
            "--dangerously-bypass-hook-trust",
        ],
    },
    Command { path: &["exec", "resume"], flags: &["--json", "--skip-git-repo-check", "--model", "-c", "--dangerously-bypass-hook-trust"] },
    Command { path: &["login"], flags: &[] },
    Command { path: &["debug", "models"], flags: &["--bundled"] },
];

pub(crate) fn commands(provider: Provider) -> &'static [Command] {
    match provider {
        Provider::Claude => CLAUDE,
        Provider::Codex => CODEX,
    }
}

/// Whether `help` lists `flag`: the flag as a word of its own, not the head
/// of a longer one (`--model` in `--model-list` is not it).
fn listed(help: &str, flag: &str) -> bool {
    help.match_indices(flag).any(|(at, _)| {
        let before = help[..at].chars().next_back();
        let after = help[at + flag.len()..].chars().next();
        let edge = |c: Option<char>| c.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        edge(before) && edge(after)
    })
}

/// The flags of `command` its help does not list.
pub(crate) fn missing(help: &str, command: &Command) -> Vec<&'static str> {
    command.flags.iter().copied().filter(|f| !listed(help, f)).collect()
}

/// The `-c` overrides Luna gives a Codex session, in their shapes: the
/// guard's hook (codex/launch.rs `guard_override`) and its MCP server
/// (`mcp_args`), aimed at nothing that answers.
fn codex_overrides() -> Vec<String> {
    let mut args = vec!["-c".to_string(), crate::codex::launch::guard_override("http://127.0.0.1:9/probe", "curl")];
    args.extend(crate::codex::launch::mcp_args("http://127.0.0.1:9/mcp"));
    args
}

/// How long one run of the binary may take: a CLI that hangs on `--help`
/// fails the probe rather than the update pass.
const RUN_FOR: Duration = Duration::from_secs(30);

/// Runs `exe` with `args` in `home`, nothing on stdin, and returns its exit
/// code and what it printed, both streams together.
fn run(provider: Provider, exe: &Path, home: &Path, args: &[String]) -> Result<(Option<i32>, String), String> {
    let mut cmd = Process::new(exe);
    cmd.args(args).current_dir(home);
    crate::pty::clean_env(&mut cmd);
    match provider {
        Provider::Claude => {
            cmd.env("CLAUDE_CONFIG_DIR", home).env("DISABLE_AUTOUPDATER", "1");
        }
        Provider::Codex => {
            cmd.env("CODEX_HOME", home);
        }
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let shown = || format!("{} {}", exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), args.join(" "));
    let mut child = cmd.spawn().map_err(|e| format!("could not run {}: {e}", shown()))?;
    let read = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut out);
            }
            out
        })
    };
    let out = read(child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let err = read(child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let deadline = Instant::now() + RUN_FOR;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{} did not finish within {}s", shown(), RUN_FOR.as_secs()));
            }
        }
    };
    let mut text = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    text.push_str(&String::from_utf8_lossy(&err.join().unwrap_or_default()));
    Ok((status.code(), text))
}

/// The probe of `exe`, staged as `version`, run in `home`, a scratch folder
/// it may write into. Err says what failed, for the bad mark and the log.
pub(crate) fn probe(provider: Provider, exe: &Path, version: &str, home: &Path) -> Result<(), String> {
    let (code, text) = run(provider, exe, home, &["--version".into()])?;
    if code != Some(0) || !listed(&text, version) {
        return Err(format!("`--version` said {:?}, not {version}", text.trim().chars().take(80).collect::<String>()));
    }
    for command in commands(provider) {
        let mut args: Vec<String> = command.path.iter().map(|s| s.to_string()).collect();
        args.push("--help".into());
        let (code, help) = run(provider, exe, home, &args)?;
        let words = if command.path.is_empty() { "the command".to_string() } else { format!("`{}`", command.path.join(" ")) };
        if code != Some(0) {
            return Err(format!("{words} --help failed: {}", help.trim().chars().take(200).collect::<String>()));
        }
        let gone = missing(&help, command);
        if !gone.is_empty() {
            return Err(format!("{words} no longer lists {}, which Luna passes it", gone.join(", ")));
        }
    }
    if provider == Provider::Codex {
        let mut args = codex_overrides();
        args.extend(["features".to_string(), "list".to_string()]);
        let (code, text) = run(provider, exe, home, &args)?;
        if code != Some(0) || text.contains("failed to load") {
            return Err(format!(
                "it refuses the hooks and MCP overrides Luna gives it: {}",
                text.trim().lines().take(3).collect::<Vec<_>>().join(" ")
            ));
        }
    }
    Ok(())
}

/// For the golden launch tests: every flag in a start's `args` is in
/// `COMMANDS` for the command it starts, so the probe asks a new version's
/// help for each flag Luna passes.
#[cfg(test)]
pub(crate) fn covers(provider: Provider, args: &[String]) -> Result<(), String> {
    let commands = commands(provider);
    // The longest path the arguments open with: `exec resume` before `exec`.
    let command = commands
        .iter()
        .filter(|c| c.path.len() <= args.len() && c.path.iter().zip(args).all(|(p, a)| p == a))
        .max_by_key(|c| c.path.len())
        .ok_or_else(|| format!("no command of {provider:?} opens {args:?}"))?;
    // What follows `--` is a prompt, not flags.
    let unlisted: Vec<&str> = args[command.path.len()..]
        .iter()
        .take_while(|a| *a != "--")
        .filter(|a| a.starts_with('-') && a.len() > 1)
        .map(|a| a.split('=').next().unwrap_or(a))
        .filter(|f| !command.flags.contains(f))
        .collect();
    match unlisted.is_empty() {
        true => Ok(()),
        false => Err(format!("{provider:?} {:?} is passed {unlisted:?}, which cli/probe.rs does not ask its help for", command.path)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--help` of every command, as the versions on the known-good list
    /// print it.
    const CLAUDE_HELP: &[(&str, &str)] = &[("", include_str!("testdata/claude-2.1.318-help.txt"))];
    const CODEX_HELP: &[(&str, &str)] = &[
        ("", include_str!("testdata/codex-0.167.0-help.txt")),
        ("resume", include_str!("testdata/codex-0.167.0-resume-help.txt")),
        ("exec", include_str!("testdata/codex-0.167.0-exec-help.txt")),
        ("exec resume", include_str!("testdata/codex-0.167.0-exec-resume-help.txt")),
        ("login", include_str!("testdata/codex-0.167.0-login-help.txt")),
        ("debug models", include_str!("testdata/codex-0.167.0-debug-models-help.txt")),
    ];

    #[test]
    fn the_checked_versions_list_every_flag_luna_passes() {
        for (provider, helps) in [(Provider::Claude, CLAUDE_HELP), (Provider::Codex, CODEX_HELP)] {
            for command in commands(provider) {
                let help = helps.iter().find(|(p, _)| *p == command.path.join(" ")).map(|(_, h)| *h).expect("a fixture per command");
                assert_eq!(missing(help, command), Vec::<&str>::new(), "{provider:?} {:?}", command.path);
            }
        }
    }

    #[test]
    fn a_flag_is_listed_only_as_a_word_of_its_own() {
        let help = "  -m, --model <MODEL>   the model\n  --modelx  other\n  --json\n";
        assert!(listed(help, "--model") && listed(help, "-m") && listed(help, "--json"));
        assert!(!listed(help, "--mod") && !listed(help, "--sandbox") && !listed("  --json-schema", "--json"));
        let gone = Command { path: &[], flags: &["--model", "--effort"] };
        assert_eq!(missing(help, &gone), ["--effort"]);
    }

    #[test]
    fn covers_takes_the_longest_command_and_flags_with_their_values() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        covers(Provider::Codex, &s(&["exec", "resume", "t-1", "--json", "-c", "x=1", "-"])).unwrap();
        let refused = covers(Provider::Codex, &s(&["exec", "resume", "t-1", "--worktree"])).unwrap_err();
        assert!(refused.contains("--worktree"), "{refused}");
        covers(Provider::Claude, &s(&["--worktree=wt-1", "--model", "opus"])).unwrap();
        assert!(covers(Provider::Claude, &s(&["--no-such-flag"])).is_err());
    }

    /// The probe end to end on a stand-in for the CLI: a script that answers
    /// `--version` and `--help` as a fixture says.
    #[cfg(unix)]
    #[test]
    fn a_stand_in_without_a_flag_or_at_another_version_fails_the_probe() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("luna-cli-stand-in-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let help = dir.join("help.txt");
        std::fs::write(&help, CLAUDE_HELP[0].1).unwrap();
        let exe = dir.join("claude");
        let script =
            format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo '2.1.318 (Claude Code)'; else cat '{}'; fi\n", help.display());
        std::fs::write(&exe, script).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        probe(Provider::Claude, &exe, "2.1.318", &dir).unwrap();
        assert!(probe(Provider::Claude, &exe, "2.1.319", &dir).unwrap_err().contains("--version"));
        std::fs::write(&help, CLAUDE_HELP[0].1.replace("--effort", "--effortless")).unwrap();
        assert!(probe(Provider::Claude, &exe, "2.1.318", &dir).unwrap_err().contains("no longer lists --effort"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
