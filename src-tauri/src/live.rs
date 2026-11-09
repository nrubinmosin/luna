//! The live suite: helpers run on the real CLIs, signed in to real accounts,
//! through the path `luna_session_spawn` takes — `agents::spawn` and its privilege
//! cap, `background::start` and `command_for`, the CLI's headless mode, Luna's
//! MCP server on the hub — and read back as `luna_session_wait` reads them. What the
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
//! a test build running beside it. No managed CLI is there, so the CLI is the
//! one the script resolved and names by its full path (cli.rs `live_cli`): the
//! test build's managed copy, or else the one on PATH.

use crate::provider::Provider;
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};

/// The Codex the live suite runs by hand (`codex sandbox`), as its sessions
/// start it (cli.rs `launch`).
fn live_codex() -> PathBuf {
    crate::cli::binary(Provider::Codex)
        .or_else(|| crate::cli::live_cli(Provider::Codex))
        .expect("no Codex: run the live suite through scripts/live-tests.ps1, which sets LUNA_LIVE_CODEX_CLI")
}
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
    // Said before any CLI is reached, though every start would refuse anyway
    // (elevation.rs): a terminal run as administrator runs the suite so too.
    if let Some(why) = crate::elevation::refusal() {
        panic!("the live suite runs no CLI from here: {why}");
    }
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

    /// A spawn as `luna_session_spawn` would take it, on this account, at low effort,
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
/// `luna_session_list` sample it, and what `luna_session_wait` then gave back.
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
    let w = crate::agents::wait(&s.caller, &s.id, "turn_done", 30, Some(0), None, &call).expect("luna_session_wait on the helper");
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
    let spawned = crate::agents::spawn(&caller, live.params(&folder, prompt, extra)).expect("luna_session_spawn");
    assert!(crate::background::has(&spawned.id), "a helper runs in the background");
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
            "Call the luna_session_list tool from the luna MCP server once, with no arguments. Then reply with exactly this word \
             and nothing else: {word}"
        ),
    );
    let w = first_turn(&s);
    assert!(w.failed.is_none(), "the turn failed: {:?}; screen: {:?}", w.failed, w.screen);
    assert!(w.reply.as_deref().unwrap_or_default().contains(&word), "reply: {:?}", w.reply);
    // mcp.rs logs every call under the session its token named, after the
    // hub has taken the token: a call refused at the door logs nothing.
    let log = std::fs::read_to_string(crate::log::path()).unwrap_or_default();
    let call = log.lines().find(|l| l.contains("[mcp]") && l.contains(&s.id) && l.contains(" luna_session_list "));
    let call = call.unwrap_or_else(|| panic!("no luna_session_list call from {} in {}", s.id, crate::log::path().display()));
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
        let mut cmd = std::process::Command::new(live_codex());
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

/// The core's endpoint served on a fresh name by the real `serve`, each
/// connection's outcome sent back: `Ok(())` for one refused before anything
/// was read, an error for one let through (it ends at a hello that is none,
/// or at its end after one).
#[cfg(windows)]
fn serve_endpoint() -> (String, std::sync::mpsc::Receiver<Result<(), String>>) {
    let name = crate::endpoint::fresh_name();
    let listener = crate::endpoint::Listener::bind(&name).expect("the endpoint");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(stream) = listener.accept() {
            let served = crate::coreproc::serve_for_test(stream).map_err(|e| e.to_string());
            if tx.send(served).is_err() {
                break;
            }
        }
    });
    (name, rx)
}

/// Where the client half of the Windows endpoint test finds the pipe's name:
/// a file beside the test executable, which a child in Codex's sandbox reads
/// as well as one outside it, whatever environment the sandbox passes on.
#[cfg(windows)]
fn probe_file() -> PathBuf {
    crate::paths::data_dir().join("live-endpoint-probe.txt")
}

/// The client half of the Windows endpoint test: started as a child of it
/// (plainly, in `codex sandbox`, at low integrity), it opens the endpoint
/// `probe_file` names as a window does and says hello. It prints `welcomed`,
/// `refused at the open: <why>` or `refused: <why>`, and fails unless
/// welcomed, so its exit code says it too.
#[test]
#[ignore = "a child process of the live endpoint test"]
#[cfg(windows)]
fn probe_client() {
    let Ok(name) = std::fs::read_to_string(probe_file()) else { return };
    let mut c = match crate::endpoint::Stream::connect(name.trim(), Duration::from_secs(5)) {
        Ok(c) => c,
        Err(e) => {
            println!("refused at the open: {e}");
            panic!("refused at the open");
        }
    };
    let _ = c.set_read_timeout(Some(Duration::from_secs(5)));
    let hello = crate::ipc::Hello { proto: crate::ipc::PROTO, role: "ctl".into() };
    let req = crate::ipc::Req { id: 1, cmd: "hello".into(), args: serde_json::to_value(hello).unwrap() };
    let answered =
        crate::ipc::send(&mut c, crate::ipc::REQ, &req).and_then(|()| crate::ipc::recv::<crate::ipc::Res>(&mut c, crate::ipc::RES));
    match answered {
        Ok(res) if res.ok.is_some() => println!("welcomed"),
        Ok(res) => panic!("refused: {:?}", res.err),
        Err(e) => {
            println!("refused: {e}");
            panic!("refused");
        }
    }
}

/// The Codex Windows sandbox cannot get into the core: under `read-only` and
/// `workspace-write` alike, Luna's own client run in it (this executable's
/// `probe_client`) is refused opening the core's pipe, whose DACL lets the
/// sandbox's restricted token nowhere near it; were it let through, `serve`
/// would refuse it before reading a byte. The same client run plainly, as a
/// window is, is welcomed by the real `serve`; run at low integrity, it is
/// refused the open by the pipe's label. No account, no login.
#[test]
#[ignore = "live: needs LUNA_LIVE=1 and the Codex CLI"]
#[cfg(windows)]
fn the_core_endpoint_refuses_a_sandboxed_codex_and_welcomes_a_plain_process() {
    if std::env::var("LUNA_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: LUNA_LIVE is not 1");
        return;
    }
    /// The probe's file goes when the test does, passed or not.
    struct Gone(PathBuf);
    impl Drop for Gone {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let (name, served) = serve_endpoint();
    std::fs::write(probe_file(), &name).expect("the probe's file");
    let _gone = Gone(probe_file());
    let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
    let probe = ["--ignored", "--exact", "live::probe_client", "--nocapture"];
    let said = |out: &std::process::Output| format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));

    // Plainly, as a window: welcomed; its connection ends past the hello.
    let out = std::process::Command::new(&exe).args(probe).output().expect("the probe runs");
    assert!(said(&out).contains("welcomed"), "a plain client is welcomed: {}", said(&out));
    let outcome = served.recv_timeout(Duration::from_secs(20)).expect("the plain client connected");
    assert!(outcome.is_err(), "the plain client got past the peer check");

    let home = crate::paths::data_dir().join("live-endpoint-home");
    std::fs::create_dir_all(&home).expect("a Codex home");
    // The `workspace-write` profile for the runner's `-P`, in a config file:
    // nested quotes in a `-c` table value do not survive the argv on Windows.
    std::fs::write(home.join("config.toml"), "[permissions.luna-peer]\nextends = \":workspace\"\n").expect("the Codex config");
    let codex = live_codex();
    let sandbox_path = crate::pty::codex_sandbox_path("read-only");
    let dir = home.to_string_lossy().into_owned();
    let unelevated = ["-c", "windows.sandbox=\"unelevated\""];
    for (label, overrides) in [
        ("read-only", vec![unelevated[0], unelevated[1], "-c", "sandbox_mode=\"read-only\""]),
        ("workspace-write", vec![unelevated[0], unelevated[1], "-P", "luna-peer", "-C", &dir]),
    ] {
        let mut cmd = std::process::Command::new(&codex);
        cmd.arg("sandbox").args(&overrides).arg("--").arg(&exe).args(probe).env("CODEX_HOME", &home).current_dir(&home);
        if let Some(p) = &sandbox_path {
            cmd.env("PATH", p);
        }
        let out = cmd.output().expect("codex sandbox runs");
        let said = said(&out);
        eprintln!("{label}: {}", said.lines().find(|l| l.starts_with("refused") || l.starts_with("welcomed")).unwrap_or("?"));
        assert!(!said.contains("welcomed"), "{label}: a sandboxed client was welcomed: {said}");
        // Either the open is refused (the DACL) and no connection comes, or
        // one comes and `serve` refuses it before the hello.
        match served.recv_timeout(Duration::from_secs(3)) {
            Err(_) => {
                assert!(said.contains("refused at the open"), "{label}: the probe neither failed to open nor reached the core: {said}")
            }
            Ok(outcome) => assert_eq!(outcome, Ok(()), "{label}: let through to the hello: {said}"),
        }
    }

    // At low integrity, as a browser's sandbox runs: refused the open.
    let code = low_integrity_run(&format!("\"{exe}\" {}", probe.join(" "))).expect("a low-integrity probe runs");
    assert_ne!(code, 0, "a low-integrity client was welcomed");
    assert!(served.recv_timeout(Duration::from_secs(3)).is_err(), "a low-integrity client reached the core");

    let _ = std::fs::remove_dir_all(&home);
}

/// Runs `command_line` at low integrity (this process's token, lowered) and
/// returns its exit code.
#[cfg(windows)]
fn low_integrity_run(command_line: &str) -> std::io::Result<u32> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        DuplicateTokenEx, SecurityImpersonation, SetTokenInformation, TokenIntegrityLevel, TokenPrimary, SID_AND_ATTRIBUTES,
        TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{
        CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject, CREATE_NO_WINDOW, INFINITE,
        PROCESS_INFORMATION, STARTUPINFOW,
    };
    let mut line: Vec<u16> = command_line.encode_utf16().chain(Some(0)).collect();
    // SAFETY: every handle opened here is closed before return; the label's
    // SID and the command line outlive the calls that read them.
    unsafe {
        let mut mine = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE | TOKEN_QUERY, &mut mine)?;
        let mut low = HANDLE::default();
        let access = TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ADJUST_DEFAULT | TOKEN_ASSIGN_PRIMARY;
        let dup = DuplicateTokenEx(mine, access, None, SecurityImpersonation, TokenPrimary, &mut low);
        let _ = CloseHandle(mine);
        dup?;
        // S-1-16-4096, the low mandatory level; SE_GROUP_INTEGRITY.
        let mut sid = [1u8, 1, 0, 0, 0, 0, 0, 16, 0, 0x10, 0, 0];
        let label = TOKEN_MANDATORY_LABEL {
            Label: SID_AND_ATTRIBUTES { Sid: windows::Win32::Security::PSID(sid.as_mut_ptr().cast()), Attributes: 0x20 },
        };
        let set = SetTokenInformation(
            low,
            TokenIntegrityLevel,
            (&label as *const TOKEN_MANDATORY_LABEL).cast(),
            size_of::<TOKEN_MANDATORY_LABEL>() as u32,
        );
        let startup = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        let mut info = PROCESS_INFORMATION::default();
        let made = set.and_then(|()| {
            CreateProcessAsUserW(
                Some(low),
                windows::core::PCWSTR::null(),
                Some(windows::core::PWSTR(line.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_NO_WINDOW,
                None,
                windows::core::PCWSTR::null(),
                &startup,
                &mut info,
            )
        });
        let _ = CloseHandle(low);
        made?;
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 0u32;
        let got = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hThread);
        let _ = CloseHandle(info.hProcess);
        got?;
        Ok(code)
    }
}

/// The Codex Linux sandbox (bubblewrap, its own user namespace) cannot get
/// into the core: under `workspace-write`, with the network off and on, Luna's
/// own client run inside it (`peer::imp::tests::probe_client`, a window's
/// hello) either cannot connect to the core's socket or is refused by the real
/// `serve` before its hello is read, and hears no welcome; the same client run
/// plainly is welcomed. Needs user namespaces (a container run with
/// `--security-opt seccomp=unconfined`). No account, no login.
#[test]
#[ignore = "live: needs LUNA_LIVE=1, the Codex CLI and user namespaces"]
#[cfg(target_os = "linux")]
fn the_core_socket_refuses_a_sandboxed_codex_and_lets_a_plain_process_in() {
    if std::env::var("LUNA_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: LUNA_LIVE is not 1");
        return;
    }
    let _ = std::fs::create_dir_all(crate::ipc::core_dir());
    let name = crate::endpoint::fresh_name();
    let listener = crate::endpoint::Listener::bind(&name).expect("the endpoint");
    // Each connection's outcome from the real handler: `Ok(())` refused before
    // anything was read, an error for one let through (it ends at its end).
    let (tx, served) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(stream) = listener.accept() {
            if tx.send(crate::coreproc::serve_for_test(stream).map_err(|e| e.to_string())).is_err() {
                break;
            }
        }
    });
    let exe = std::env::current_exe().unwrap();
    let probe = |mut cmd: std::process::Command| {
        cmd.args(["--exact", "peer::imp::tests::probe_client", "--ignored", "--nocapture"]).env("LUNA_PEER_PROBE", &name);
        let out = cmd.output().expect("the probe runs");
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    };
    let plain = probe(std::process::Command::new(&exe));
    assert!(plain.contains("welcomed"), "a plain client is welcomed: {plain}");
    let outcome = served.recv_timeout(Duration::from_secs(20)).expect("the plain probe connected");
    assert!(outcome.is_err(), "the plain client got past the peer check");

    let home = crate::paths::data_dir().join("live-socket-home");
    std::fs::create_dir_all(&home).expect("a Codex home");
    std::fs::write(
        home.join("config.toml"),
        "[permissions.luna-ws]\nextends = \":workspace\"\n\n[permissions.luna-ws-net]\nextends = \":workspace\"\n\n[permissions.luna-ws-net.network]\nenabled = true\n",
    )
    .expect("the Codex config");
    let codex = live_codex();
    let dir = home.to_string_lossy().into_owned();
    for profile in ["luna-ws", "luna-ws-net"] {
        let mut cmd = std::process::Command::new(&codex);
        cmd.args(["sandbox", "-P", profile, "-C", &dir, "--"]).arg(&exe).env("CODEX_HOME", &home).current_dir(&home);
        let said = probe(cmd);
        assert!(!said.contains("welcomed"), "{profile}: a sandboxed client was welcomed: {said}");
        match served.recv_timeout(Duration::from_secs(10)) {
            Ok(outcome) => {
                eprintln!("{profile}: connected, {}", if outcome.is_ok() { "refused before the hello" } else { "LET THROUGH" });
                assert_eq!(outcome, Ok(()), "{profile}: let through to the hello: {said}");
            }
            Err(_) => {
                eprintln!("{profile}: could not connect");
                assert!(said.contains("refused at the connect"), "{profile}: the probe neither connected nor failed to: {said}");
            }
        }
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
