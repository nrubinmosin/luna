//! The live suite: helpers run on the real CLIs, signed in to real accounts,
//! through the path `luna_spawn` takes — `agents::spawn` and its privilege
//! cap, `background::start` and `command_for`, the CLI's headless mode, Luna's
//! MCP server on the hub — and read back as `luna_wait` reads them. What the
//! deterministic tests stand in for with fixtures (a CLI's stream, the flags
//! it accepts, its MCP client) is the CLI's own here, so a CLI release that
//! changes any of it shows.
//!
//! Every test is ignored, and does anything only when run with `--ignored` and
//! `LUNA_LIVE=1` (scripts/live-tests.ps1 does both): a turn spends the
//! account's quota, needs the network and a login, and its words are a
//! model's, so this never runs in CI (docs/testing.md). The accounts are
//! named by LUNA_LIVE_CLAUDE_ACCOUNT and LUNA_LIVE_CODEX_ACCOUNT, and must be
//! throwaway copies in a checkout's test-build/accounts (scripts/test-accounts.ps1)
//! reached through no link: the CLI writes into its account folder, and a
//! refresh of its login there can sign out the account it was copied from.
//!
//! Run from its own folder (test-build/live-tests), the test executable's data
//! dir is that folder: its hub, settings and helper records share nothing with
//! a test build running beside it. No managed CLI is there, so the CLI is
//! found on PATH, as Luna finds one before its first download; the script puts
//! the test build's managed copy first on it.

use crate::provider::Provider;
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

/// How long one test waits on its helper's turn, unless LUNA_LIVE_TIMEOUT_S
/// says otherwise: a one-line answer takes seconds, and a CLI that has not
/// answered in this has a problem worth failing on.
const TIME_BOX: Duration = Duration::from_secs(240);

/// A test account as the environment names it, checked.
struct Live {
    provider: Provider,
    /// Its folder: `<test-build>/accounts/<anthropic|openai>/<name>`.
    account: PathBuf,
    name: String,
    test_build: PathBuf,
}

/// The account for `provider`, or None when LUNA_LIVE is not 1: the test then
/// does nothing, so `--include-ignored` alone never spends a token. Set, a
/// missing or wrong account is a failure, not a skip.
fn live(provider: Provider) -> Option<Live> {
    if std::env::var("LUNA_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: LUNA_LIVE is not 1");
        return None;
    }
    let var = match provider {
        Provider::Claude => "LUNA_LIVE_CLAUDE_ACCOUNT",
        Provider::Codex => "LUNA_LIVE_CODEX_ACCOUNT",
    };
    let path = std::env::var(var)
        .unwrap_or_else(|_| panic!("LUNA_LIVE=1 needs {var}: an account folder in test-build/accounts/{}", provider.accounts_dir()));
    Some(test_account(provider, Path::new(&path)).unwrap_or_else(|e| panic!("{var}: {e}")))
}

/// `path` as a test account of `provider`: absolute, with no `..`, laid out as
/// `.../test-build/accounts/<provider's folder>/<name>`, and no link on the way
/// from test-build down, which could lead the CLI's writes to a real account.
fn test_account(provider: Provider, path: &Path) -> Result<Live, String> {
    let shown = path.display();
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir)) {
        return Err(format!("{shown} is not an absolute path without . or .."));
    }
    let named = |p: &Path, want: &str| p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(want));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or_else(|| format!("{shown} names no account"))?;
    let provider_dir = path.parent().filter(|p| named(p, provider.accounts_dir()));
    let accounts = provider_dir.and_then(Path::parent).filter(|p| named(p, "accounts"));
    let test_build = accounts.and_then(Path::parent).filter(|p| named(p, "test-build"));
    let (Some(provider_dir), Some(accounts), Some(test_build)) = (provider_dir, accounts, test_build) else {
        return Err(format!("{shown} is not in test-build/accounts/{}: only a throwaway test account is used", provider.accounts_dir()));
    };
    for p in [test_build, accounts, provider_dir, path] {
        let meta = std::fs::symlink_metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
        if is_link(&meta) {
            return Err(format!("{} is a junction or symlink: a test account is a folder of its own", p.display()));
        }
        if !meta.is_dir() {
            return Err(format!("{} is not a folder", p.display()));
        }
    }
    Ok(Live { provider, account: path.to_path_buf(), name, test_build: test_build.to_path_buf() })
}

fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // A junction is a reparse point too, whatever std calls it.
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

fn time_box() -> Duration {
    std::env::var("LUNA_LIVE_TIMEOUT_S").ok().and_then(|s| s.parse().ok()).map_or(TIME_BOX, Duration::from_secs)
}

impl Live {
    /// Luna set up as a core would be for a spawn: the hub listening, the
    /// accounts read from this account's test-build/accounts, and helpers held
    /// to their caller (the cap the refusal tests count on).
    fn luna(&self) {
        crate::hub::start();
        let root = self.account.parent().and_then(Path::parent).expect("checked in test_account");
        crate::settings::update(|s| {
            s.accounts_root = Some(root.to_string_lossy().into_owned());
            s.helpers_above_caller = false;
        })
        .expect("the live data dir's settings");
        let listed = crate::accounts::list_accounts().unwrap_or_default();
        assert!(
            listed.iter().any(|a| a.provider == self.provider && a.name == self.name),
            "Luna does not list {} among its accounts",
            self.account.display()
        );
    }

    /// A project folder of its own for one test, outside the data dir (a
    /// helper is refused there): test-build/live-work/<id>.
    fn work_folder(&self, id: &str) -> String {
        let dir = self.test_build.join("live-work").join(id);
        std::fs::create_dir_all(&dir).expect("the work folder");
        dir.to_string_lossy().into_owned()
    }

    /// A registered session to spawn from, in `settings`' mode, with tools,
    /// and in a turn as far as the agent layer can tell: a `wait` lets go once
    /// its caller is no live session (agents/ask.rs `live`). A helper with no
    /// process stands in, as in the agent layer's own tests.
    fn caller(&self, folder: &str, settings: Value) -> String {
        let id = format!("live-caller-{}", crate::background::mint_id());
        let account = self.account.to_string_lossy();
        crate::agents::register(&id, self.provider, folder, &account, None, true, settings);
        crate::background::test_helper(&id, &account, None, true);
        id
    }

    /// A spawn as `luna_spawn` would take it, on this account, at low effort,
    /// kept until the test deletes it. The model is the account's default
    /// unless LUNA_LIVE_CLAUDE_MODEL / LUNA_LIVE_CODEX_MODEL names one.
    fn params(&self, folder: &str, prompt: &str, extra: Value) -> crate::agents::SpawnParams {
        let model_var = match self.provider {
            Provider::Claude => "LUNA_LIVE_CLAUDE_MODEL",
            Provider::Codex => "LUNA_LIVE_CODEX_MODEL",
        };
        let mut v = json!({
            "provider": self.provider.as_str(),
            "account": self.name,
            "folder": folder,
            "prompt": prompt,
            "name": "live test",
            "effort": "low",
            "autoDelete": false,
        });
        if let Ok(model) = std::env::var(model_var) {
            v["model"] = json!(model);
        }
        for (k, x) in extra.as_object().into_iter().flatten() {
            v[k] = x.clone();
        }
        serde_json::from_value(v).expect("spawn params")
    }
}

/// The least a caller may run in to spawn a helper in its provider's own
/// lowest asking mode, and that mode for the helper.
fn modest(provider: Provider) -> (Value, Value) {
    match provider {
        Provider::Claude => (json!({ "permissionMode": "default" }), json!({ "permissionMode": "default" })),
        Provider::Codex => (json!({ "sandbox": "read-only", "approval": "never" }), json!({ "sandbox": "read-only" })),
    }
}

/// A word the model can only have from this test's prompt.
fn nonce() -> String {
    format!("LUNA-LIVE-{}", crate::agents::mint()[..8].to_uppercase())
}

/// The helper and its caller go when the test does, passed or not.
struct Spawned {
    caller: String,
    id: String,
    folder: String,
}

impl Drop for Spawned {
    fn drop(&mut self) {
        let _ = crate::agents::kill(&self.caller, &self.id);
        if let Err(e) = crate::agents::delete(&self.caller, &self.id, true, true) {
            eprintln!("could not delete live helper {}: {e}", self.id);
        }
        crate::background::gone_for_test(&self.caller);
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// What the turn showed while it ran, sampled as the power rules and
/// `luna_list` sample it, and what `luna_wait` then gave back.
struct Turn {
    states: Vec<crate::activity::Turn>,
    outcome: &'static str,
    turns_ended: u32,
    reply: Option<String>,
    failed: Option<String>,
    screen: Option<String>,
}

/// Waits out the first turn of `s` within the time box, noting each change
/// of its turn state on the way.
fn first_turn(s: &Spawned) -> Turn {
    let deadline = Instant::now() + time_box();
    let mut states = Vec::new();
    loop {
        let t = crate::activity::turn_state(&s.id).expect("the helper is known");
        if states.last() != Some(&t.turn) {
            states.push(t.turn);
        }
        if t.turns_ended >= 1 {
            break;
        }
        assert!(Instant::now() < deadline, "no turn ended within {:?}; went through {states:?}", time_box());
        std::thread::sleep(Duration::from_millis(100));
    }
    let call = crate::mcp::Call::new();
    let w = crate::agents::wait(&s.caller, &s.id, "turn_done", 30, Some(0), &call).expect("luna_wait on the helper");
    Turn { states, outcome: w.outcome, turns_ended: w.turns_ended, reply: w.reply, failed: w.failed, screen: w.screen }
}

fn spawn(live: &Live, extra: Value, prompt: &str) -> Spawned {
    live.luna();
    let id = crate::background::mint_id();
    let folder = live.work_folder(&id);
    let (caller_mode, helper_mode) = modest(live.provider);
    let caller = live.caller(&folder, caller_mode);
    let mut extra = extra;
    for (k, v) in helper_mode.as_object().into_iter().flatten() {
        extra[k] = v.clone();
    }
    let spawned = crate::agents::spawn(&caller, live.params(&folder, prompt, extra)).expect("luna_spawn");
    assert!(spawned.started, "a background helper starts at once");
    Spawned { caller, id: spawned.id, folder }
}

/// A turn on the real CLI ends with the answer asked for, and on the way its
/// stream says what Luna reads off it: the conversation's id, the model, the
/// turn busy and then over.
fn a_turn_completes_with_an_answer(provider: Provider) {
    let Some(live) = live(provider) else { return };
    let word = nonce();
    let s = spawn(&live, json!({}), &format!("Reply with exactly this word and nothing else, using no tools: {word}"));
    let w = first_turn(&s);
    assert_eq!(w.outcome, "turn_done", "{:?} {:?}", w.failed, w.screen);
    assert!(w.failed.is_none(), "the turn failed: {:?}; screen: {:?}", w.failed, w.screen);
    assert_eq!(w.turns_ended, 1);
    let reply = w.reply.as_deref().unwrap_or_default();
    assert!(reply.contains(&word), "the reply does not carry {word}: {reply:?}");

    use crate::activity::Turn::{Busy, Idle};
    assert_eq!(w.states.first(), Some(&Busy), "a spawn is a turn under way: {:?}", w.states);
    assert_eq!(w.states.last(), Some(&Idle), "and over once it has ended: {:?}", w.states);
    assert!(crate::background::session_id(&s.id).is_some(), "the stream named the conversation");
    let signs = crate::background::signs(&s.id).expect("the helper's signs");
    assert!(signs.resolved_model.is_some(), "the stream named the model it ran on");
    assert!(crate::background::transcript(&s.id).is_some_and(|p| p.is_file()), "the CLI's transcript is on disk");
}

/// A helper with tools reaches Luna's MCP server on the token in its
/// environment: the hub took the call as this helper's, and the answer came
/// back to the model, which then said the word.
fn an_mcp_call_authenticates(provider: Provider) {
    let Some(live) = live(provider) else { return };
    let word = nonce();
    let s = spawn(
        &live,
        json!({ "tools": true }),
        &format!(
            "Call the luna_list tool from the luna MCP server once, with no arguments. Then reply with exactly this word \
             and nothing else: {word}"
        ),
    );
    let w = first_turn(&s);
    assert!(w.failed.is_none(), "the turn failed: {:?}; screen: {:?}", w.failed, w.screen);
    assert!(w.reply.as_deref().unwrap_or_default().contains(&word), "reply: {:?}", w.reply);
    // mcp.rs logs every call under the session its token named, after the
    // hub has taken the token: a call refused at the door logs nothing.
    let log = std::fs::read_to_string(crate::log::path()).unwrap_or_default();
    let call = log.lines().find(|l| l.contains("[mcp]") && l.contains(&s.id) && l.contains(" luna_list "));
    let call = call.unwrap_or_else(|| panic!("no luna_list call from {} in {}", s.id, crate::log::path().display()));
    assert!(!call.contains(" failed in "), "{call}");
}

/// A caller in the lowest mode asks for a helper in the highest: refused
/// before anything starts, with the reason, and no CLI run for it.
fn an_over_privileged_spawn_is_refused(provider: Provider) {
    let Some(live) = live(provider) else { return };
    live.luna();
    let id = crate::background::mint_id();
    let folder = live.work_folder(&id);
    let (caller_mode, over) = match provider {
        Provider::Claude => (json!({ "permissionMode": "plan" }), json!({ "permissionMode": "bypassPermissions" })),
        Provider::Codex => (json!({ "sandbox": "read-only", "approval": "never" }), json!({ "sandbox": "danger-full-access" })),
    };
    let caller = live.caller(&folder, caller_mode);
    let before = crate::background::background_helpers().len();
    let refused = crate::agents::spawn(&caller, live.params(&folder, "Say hello.", over)).map(|s| s.id);
    let err = refused.expect_err("an over-privileged spawn is refused");
    assert!(err.contains("more privilege than its caller"), "{err}");
    assert_eq!(crate::background::background_helpers().len(), before, "no helper was started");
    crate::background::gone_for_test(&caller);
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
#[ignore = "live: needs LUNA_LIVE=1 and a signed-in test account"]
fn claude_a_turn_completes_with_an_answer() {
    a_turn_completes_with_an_answer(Provider::Claude);
}

#[test]
#[ignore = "live: needs LUNA_LIVE=1 and a signed-in test account"]
fn codex_a_turn_completes_with_an_answer() {
    a_turn_completes_with_an_answer(Provider::Codex);
}

#[test]
#[ignore = "live: needs LUNA_LIVE=1 and a signed-in test account"]
fn claude_an_mcp_call_authenticates() {
    an_mcp_call_authenticates(Provider::Claude);
}

#[test]
#[ignore = "live: needs LUNA_LIVE=1 and a signed-in test account"]
fn codex_an_mcp_call_authenticates() {
    an_mcp_call_authenticates(Provider::Codex);
}

#[test]
#[ignore = "live: needs LUNA_LIVE=1 and a signed-in test account"]
fn claude_an_over_privileged_spawn_is_refused() {
    an_over_privileged_spawn_is_refused(Provider::Claude);
}

#[test]
#[ignore = "live: needs LUNA_LIVE=1 and a signed-in test account"]
fn codex_an_over_privileged_spawn_is_refused() {
    an_over_privileged_spawn_is_refused(Provider::Codex);
}

/// What a Codex session in each of Luna's sandboxes can read of Luna's
/// `core/`, tried in Codex's own sandbox runner (`codex sandbox`, which needs
/// no login, so no account either) with the overrides a session is given.
/// Asserted as the threat model has it (F3), so that a change on either side
/// shows: off Windows the read-only sandboxes are refused it and
/// `workspace-write` is not; on Windows every sandbox reads it, and the deny
/// Luna leaves out there is still one Codex refuses to run under. A probe file
/// stands in for `core.json`; what is read is counted, never shown.
#[test]
#[ignore = "live: needs LUNA_LIVE=1 and the Codex CLI"]
fn codex_sandboxes_reach_the_core_folder_as_the_threat_model_says() {
    if std::env::var("LUNA_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: LUNA_LIVE is not 1");
        return;
    }
    /// The probe and the Codex home go when the test does, passed or not.
    struct Gone(PathBuf, PathBuf);
    impl Drop for Gone {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            let _ = std::fs::remove_dir_all(&self.1);
        }
    }
    let core = crate::ipc::core_dir();
    crate::secret::private_dir(&core).expect("core/");
    let probe = core.join(format!("probe-{}.json", crate::agents::mint()[..8].to_lowercase()));
    // A Codex home of its own: only Luna's overrides decide the sandbox.
    let home = crate::paths::data_dir().join("live-codex-home");
    let _gone = Gone(probe.clone(), home.clone());
    crate::secret::write_private(&probe, b"{\"probe\":\"not a token\"}").expect("the probe file");
    std::fs::create_dir_all(&home).expect("the Codex home");
    let path = probe.to_string_lossy().into_owned();
    let reader: Vec<String> = if cfg!(windows) {
        // A cmdlet, and "refused" for a denied access only: in the sandbox
        // Windows PowerShell runs in ConstrainedLanguage, which refuses .NET
        // method calls whatever the file's access.
        let quoted = path.replace('\'', "''");
        let script = format!(
            "try {{ 'read ok ' + (Get-Content -LiteralPath '{quoted}' -Encoding Byte -ReadCount 0 -ErrorAction Stop).Length }} \
             catch [System.UnauthorizedAccessException] {{ 'refused' }} catch {{ 'failed ' + $_.FullyQualifiedErrorId }}"
        );
        ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", &script].map(String::from).to_vec()
    } else {
        let script = r#"if n=$(wc -c < "$1" 2>/dev/null); then echo "read ok $n"; else echo refused; fi"#;
        ["sh", "-c", script, "sh", &path].map(String::from).to_vec()
    };
    let run = |overrides: Vec<String>, sandbox: &str| {
        let mut cmd = std::process::Command::new(crate::cli::binary(Provider::Codex));
        cmd.arg("sandbox")
            .args(&overrides)
            .args(crate::codex::launch::codex_windows_sandbox(sandbox).map(|sb| ["-c".to_string(), sb]).into_iter().flatten());
        cmd.arg("--").args(&reader).env("CODEX_HOME", &home).current_dir(&home);
        if let Some(p) = crate::pty::codex_sandbox_path(sandbox) {
            cmd.env("PATH", p);
        }
        let out = cmd.output().expect("codex sandbox runs");
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    };
    let read = |said: &str| said.lines().any(|l| l.trim().starts_with("read ok "));
    let refused = |said: &str| said.lines().any(|l| l.trim() == "refused");
    for sandbox in ["danger-full-access", "read-only", crate::codex::launch::READ_ONLY_NET, "workspace-write"] {
        let said = run(crate::pty::codex_sandbox_args(sandbox), sandbox);
        eprintln!(
            "{sandbox}: {}",
            if read(&said) {
                "read"
            } else if refused(&said) {
                "refused"
            } else {
                "neither"
            }
        );
        let denied = crate::codex::launch::denies_core(sandbox, cfg!(windows));
        assert!(if denied { refused(&said) } else { read(&said) }, "{sandbox}: {said}");
    }
    if cfg!(windows) {
        let dir = core.to_string_lossy();
        let said = run(crate::codex::launch::profile_args("luna-live-probe", false, Some(&dir)), "read-only");
        assert!(!read(&said) && said.contains("elevated"), "Codex runs a deny on Windows now; Luna can give it there: {said}");
    }
}

/// Accepts one connection on `listener` (nonblocking) within `secs`, or None.
#[cfg(windows)]
fn accept_within(listener: &std::net::TcpListener, secs: u64) -> Option<std::net::TcpStream> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Some(stream),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(_) => return None,
        }
    }
}

/// The Windows peer check (peer.rs) turns a connection from a Codex session in
/// its sandbox away before the hello, while a plain local process — as a
/// window is — is let through. Proven against the real Codex sandbox: a
/// sandboxed `curl` owns a restricted token, an unsandboxed one and the test
/// process own the user's own. `read-only` and `workspace-write` both, since
/// the unelevated sandbox restricts the token for either (its write access is
/// an ACL grant, not a looser token). No account, no login.
#[test]
#[ignore = "live: needs LUNA_LIVE=1 and the Codex CLI"]
#[cfg(windows)]
fn the_windows_peer_check_refuses_a_sandboxed_codex() {
    use std::net::{TcpListener, TcpStream};
    use std::process::Stdio;
    if std::env::var("LUNA_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: LUNA_LIVE is not 1");
        return;
    }
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("a loopback listener");
    listener.set_nonblocking(true).expect("nonblocking");
    let port = listener.local_addr().unwrap().port();

    // A plain local process, as a window is: let through. The test process
    // owns the client end, an unrestricted token of the core's integrity.
    let mine = TcpStream::connect(("127.0.0.1", port)).expect("the self connection");
    let server = accept_within(&listener, 5).expect("the self connection is accepted");
    assert_eq!(crate::peer::refuse_reason(&server), None, "a plain local process is let in");
    drop((mine, server));

    let home = crate::paths::data_dir().join("live-peer-home");
    std::fs::create_dir_all(&home).expect("a Codex home");
    // The `workspace-write` profile for the runner's `-P`, defined in a config
    // file rather than a `-c` inline table: nested quotes in a table value do
    // not survive the argv on Windows, where a `permissions.<p>.extends=…`
    // key, as Luna's own profiles pass it, does.
    std::fs::write(home.join("config.toml"), "[permissions.luna-peer]\nextends = \":workspace\"\n").expect("the Codex config");
    let codex = crate::cli::binary(Provider::Codex);
    let url = format!("http://127.0.0.1:{port}/");
    let sandbox_path = crate::pty::codex_sandbox_path("read-only");

    // `--noproxy *`: the sandbox's "no network" only points proxy variables at
    // a dead port, which curl would otherwise follow instead of the listener;
    // a raw socket to loopback the sandbox still allows (cli-compatibility.md).
    let client = ["curl.exe", "--noproxy", "*", "-s", "-m", "3", &url];

    // Runs `codex sandbox <overrides> -- curl <url>`: curl owns the socket
    // under the sandbox's restricted token. The outer Option is whether a
    // connection came at all; the inner is the peer check's verdict on it
    // (Some = refused). A connection that never comes is not a verdict:
    // Codex's unelevated Windows sandbox accrues machine-wide network state and,
    // several runs into one process (a bare `--ignored` run of these tests),
    // stops reaching loopback under `workspace-write` — and `codex sandbox
    // uninstall`, which would clear it, needs an administrator. So that sub-check
    // is skipped, not failed; a single `--exact` run (how these are meant to be
    // run) connects and asserts.
    let probe = |overrides: Vec<String>, label: &str| -> Option<Option<String>> {
        let mut cmd = std::process::Command::new(&codex);
        cmd.arg("sandbox").args(&overrides).arg("--").args(client);
        cmd.env("CODEX_HOME", &home).current_dir(&home);
        if let Some(p) = &sandbox_path {
            cmd.env("PATH", p);
        }
        let mut child = cmd.stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("codex sandbox runs");
        let got = accept_within(&listener, 20).map(|s| crate::peer::refuse_reason(&s));
        let _ = child.wait();
        if got.is_none() {
            eprintln!("skipped {label}: the sandbox did not reach loopback (Codex sandbox network state; run this test on its own)");
        }
        got
    };

    // A connection that came is asserted refused (the inner is Some); one that
    // did not come is skipped (`probe` said why). At least one mode must
    // connect, so the test is not silently a no-op.
    let assert_refused = |got: Option<Option<String>>, label: &str| -> bool {
        match got {
            Some(verdict) => {
                assert!(verdict.is_some(), "{label} sandbox is refused; verdict was allow");
                eprintln!("{label} sandbox: {}", verdict.unwrap());
                true
            }
            None => false,
        }
    };
    let dir = home.to_string_lossy().into_owned();
    let read_only =
        probe(vec!["-c".into(), "windows.sandbox=\"unelevated\"".into(), "-c".into(), "sandbox_mode=\"read-only\"".into()], "read-only");
    let ro = assert_refused(read_only, "read-only");
    // workspace-write through the runner names the config's profile with `-P`.
    let ws = probe(
        vec!["-c".into(), "windows.sandbox=\"unelevated\"".into(), "-P".into(), "luna-peer".into(), "-C".into(), dir],
        "workspace-write",
    );
    let ws = assert_refused(ws, "workspace-write");
    assert!(ro || ws, "neither sandbox reached loopback; run this test on its own");

    // An unsandboxed curl, the user's own token, as any local process: let in.
    let mut child =
        std::process::Command::new(client[0]).args(&client[1..]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("curl runs");
    let plain = accept_within(&listener, 10).map(|s| crate::peer::refuse_reason(&s));
    let _ = child.wait();
    assert_eq!(plain, Some(None), "an unsandboxed local process is let in");

    let _ = std::fs::remove_dir_all(&home);
}

/// The peer check enforces through the real connection handler, not only
/// `refuse_reason`: without it a sandboxed Codex with the token would reach
/// dispatch. Driven through `coreproc::serve_for_test`, so deleting the check
/// from `serve` fails this test. A plain in-process client (as a window is)
/// with a valid hello is welcomed; a sandboxed Codex is refused before the
/// hello — with the check gone `serve` reads its HTTP as a frame and errors
/// (`Err`) instead of returning `Ok`.
#[test]
#[ignore = "live: needs LUNA_LIVE=1 and the Codex CLI"]
#[cfg(windows)]
fn serve_welcomes_a_plain_client_and_refuses_a_sandboxed_one() {
    use std::net::{TcpListener, TcpStream};
    use std::process::Stdio;
    if std::env::var("LUNA_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: LUNA_LIVE is not 1");
        return;
    }
    let token = crate::ipc::mint_token();
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("a loopback listener");
    let port = listener.local_addr().unwrap().port();

    // (a) A plain in-process client — the user's own token, as a window has —
    // sends a valid hello and is welcomed.
    let hello_token = token.clone();
    let client = std::thread::spawn(move || {
        let mut c = TcpStream::connect(("127.0.0.1", port)).expect("the client connects");
        let hello = crate::ipc::Hello { proto: crate::ipc::PROTO, token: hello_token, role: "ctl".into() };
        let req = crate::ipc::Req { id: 1, cmd: "hello".into(), args: serde_json::to_value(hello).unwrap() };
        crate::ipc::send(&mut c, crate::ipc::REQ, &req).expect("the hello is sent");
        let res: crate::ipc::Res = crate::ipc::recv(&mut c, crate::ipc::RES).expect("a welcome comes back");
        res.ok.is_some()
    });
    let (plain, _) = listener.accept().expect("the plain client is accepted");
    // Returns Err once the client disconnects (requests reads EOF); the
    // welcome the client read is what proves acceptance.
    let _ = crate::coreproc::serve_for_test(plain, &token);
    assert!(client.join().unwrap(), "a plain local client with a valid hello is welcomed");

    // (b) A sandboxed Codex connecting (dumb curl) is refused before the hello.
    let home = crate::paths::data_dir().join("live-serve-home");
    std::fs::create_dir_all(&home).expect("a Codex home");
    let codex = crate::cli::binary(Provider::Codex);
    let url = format!("http://127.0.0.1:{port}/");
    let sandbox_path = crate::pty::codex_sandbox_path("read-only");
    let mut cmd = std::process::Command::new(&codex);
    cmd.arg("sandbox")
        .args(["-c", "windows.sandbox=\"unelevated\"", "-c", "sandbox_mode=\"read-only\""])
        .arg("--")
        .args(["curl.exe", "--noproxy", "*", "-s", "-m", "5", &url])
        .env("CODEX_HOME", &home)
        .current_dir(&home);
    if let Some(p) = &sandbox_path {
        cmd.env("PATH", p);
    }
    let mut child = cmd.stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("codex sandbox runs");
    let sandboxed = accept_within(&listener, 20);
    let _ = child.wait();
    // The sandbox not reaching loopback is a Codex machine-wide-state issue in a
    // crowded `--ignored` run, not a verdict; skip rather than fail (run this
    // test on its own to exercise it). The plain-client half above still ran.
    match sandboxed {
        Some(stream) => {
            let served = crate::coreproc::serve_for_test(stream, &token);
            assert!(
                served.is_ok(),
                "serve refuses the sandboxed peer before the hello; with the check gone it reads the HTTP as a frame and errors"
            );
        }
        None => eprintln!("skipped the sandboxed half: the sandbox did not reach loopback (run this test on its own)"),
    }

    let _ = std::fs::remove_dir_all(&home);
}

/// What the suite takes for a test account, checked without one: the guard
/// is what keeps a live run off the accounts a person uses.
#[test]
fn only_a_folder_in_test_build_accounts_is_a_test_account() {
    let root = std::env::temp_dir().join(format!("luna-live-guard-{}", std::process::id()));
    let account = root.join("test-build").join("accounts").join("anthropic").join("one");
    std::fs::create_dir_all(&account).unwrap();
    let elsewhere = root.join("accounts").join("anthropic").join("one");
    std::fs::create_dir_all(&elsewhere).unwrap();

    let ok = test_account(Provider::Claude, &account).expect("a test account");
    assert_eq!((ok.name.as_str(), ok.test_build.as_path()), ("one", root.join("test-build").as_path()));
    assert!(test_account(Provider::Codex, &account).is_err(), "an Anthropic folder is not a Codex account");
    assert!(test_account(Provider::Claude, &elsewhere).is_err(), "not under test-build");
    assert!(test_account(Provider::Claude, Path::new("test-build/accounts/anthropic/one")).is_err(), "relative");
    let dotted = root.join("test-build").join("accounts").join("anthropic").join("..").join("anthropic").join("one");
    assert!(test_account(Provider::Claude, &dotted).is_err(), "with ..");
    assert!(test_account(Provider::Claude, &account.with_file_name("missing")).is_err(), "not there");

    #[cfg(unix)]
    {
        let link = account.with_file_name("linked");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        let err = test_account(Provider::Claude, &link).err().unwrap_or_default();
        assert!(err.contains("junction or symlink"), "{err}");
    }
    let _ = std::fs::remove_dir_all(&root);
}
