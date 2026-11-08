# Testing Luna

Luna drives two third-party CLIs (Claude Code and Codex) that change on their
own release schedule, through a model whose words are never the same twice. So
the tests come in three levels, each answering a different question at a
different cost. CI runs the first two; the third is run by hand.

| Level | What it answers | Runs where | Costs |
|---|---|---|---|
| 1. Deterministic | Is Luna's own logic right? | `gate.ps1`, CI | seconds, nothing external |
| 2. Integration | Do Luna's parts fit, and does it start the CLIs exactly as intended? | `gate.ps1`, `windows-tests.ps1`, CI; the test build by hand | minutes, no accounts |
| 3. Live | Do today's real CLIs, signed in, still do what Luna counts on? | `live-tests.ps1`, by hand only | quota on test accounts, network |

Pick the lowest level that can catch the bug. A wrong state transition, a parse
of a CLI event, a privilege rule: level 1. A flag the CLI is launched with, the
hub's HTTP framing, a window that will not open: level 2. A CLI release that
renamed a stream event or stopped honoring a flag: only level 3 sees it, and
what it finds becomes a fixture at level 1 or a golden at level 2.

## 1. Deterministic: unit and component tests

- **Rust** unit tests sit beside the code (`#[cfg(test)] mod tests`). They feed
  Luna recorded CLI events (`background/events.rs`), saved state across a
  simulated core restart (`background/testkit.rs`), and helpers with no process
  behind them (`agents/test_support.rs`). No network, no CLI binary, no account.
- **Frontend** tests are Vitest (`pnpm test`), on the stores and components,
  with the demo data in `src/dev/demo/` where a whole screen is needed.
- **Scripts**: `pnpm test:scripts` (Node's test runner, `scripts/*.test.mjs`),
  `scripts/check-commit-msg.test.sh`, and `./scripts/test-accounts.ps1 self-test`.

Run them:

```powershell
./scripts/gate.ps1                         # everything below in Docker, as CI does
./scripts/gate.ps1 -Group rust             # the Rust half only (Linux)
./scripts/windows-tests.ps1 -Build -Run    # the Rust tests as Windows executables, run here
```

## 2. Integration

- **Golden launch tests** pin the exact command line and environment Luna
  starts each CLI with, for a chat in a pane (`pty/start.rs`) and for a
  background helper (`background/command.rs`), with run-specific values (ports,
  tokens, data-dir paths) redacted by `golden.rs`. A change to a flag shows as a
  diff to review, not as a surprise in production.
- **Hub and listener tests** (`hub.rs`, `mcp.rs`) run the real loopback
  listener: framing, size and time limits, a trickling or silent client,
  saturation, a posted hook reaching the activity tracker, MCP requests and
  their auth.
- **Test-build smoke**: `./scripts/test-build.ps1 -Run -Background` builds and
  starts the test flavor (`test-build/luna-test.exe`), which keeps its log,
  settings and downloaded CLIs in `test-build/` and runs beside the Luna in use
  without touching it. `-DevCaller` also registers a scripted MCP caller, so
  Luna's tools can be exercised over HTTP (`agents/registry.rs`
  `register_dev_caller`). `./scripts/screenshots.ps1` renders the UI headless on
  the demo data.

The golden and hub tests run in the gate and in CI with the rest of the unit
tests. The test-build smoke is by hand.

### The owner's desktop is off limits

The test build runs on a desktop someone is using. With `-Background` its
window stays off screen, unfocused and off the taskbar, and keeps working.
Drive it only by means aimed at the test build's own windows: WebDriver on its
WebView (preferred), messages to its own window handles, UI Automation patterns
on its own elements. Never anything global or anything that takes focus: no
synthetic input (`SendInput`, `keybd_event`, `mouse_event`, `SendKeys`, cursor
moves), no `SetForegroundWindow`, nothing aimed at a window that is not the test
build's.

## Fuzzing

Luna reads both CLIs' transcripts, and a prompt-injected agent decides much of
what lands in them, so the readers take untrusted input. Each provider's are
pure functions of a file's bytes, `claude::session::read_transcript` and
`codex::session::read_rollout`: every part of a transcript the rest of Luna
reads (the conversation, opening prompt, titles, context, model, last words,
API error, pending tool, due time, the Codex turn scan), by the same functions
the file-based readers call, with no file, lock or cache between. The fixture
tests (`claude/session/fixtures.rs`, `codex/session/fixtures.rs`) hold both
ways of reading to one golden per file in `src-tauri/fixtures/transcripts/`
(`LUNA_BLESS=1` rewrites the goldens, in a container that can write the
checkout; review the diff).

`src-tauri/fuzz` is a cargo-fuzz crate, a workspace of its own on a nightly
pinned for it alone, with one target per provider, `claude_transcript` and
`codex_rollout`: each corpus is seeded from its provider's fixtures, and each
has a dictionary of the keys and values its readers match on
(`src-tauri/fuzz/*.dict`). Beyond not panicking, with overflow checks on, a
target checks that:

- the reading stops where the complete lines do, and a reader that goes on
  from its cursor, after the file grew, gets the conversation a read from the
  top does, `after_cli` included (Claude Code);
- every message, title, model, question, failure and timestamp it hands over
  is part of a string in the input (or a line the reader writes itself, such
  as `[tool: Name]`), and messages are trimmed and never empty;
- the moment a process was spawned bears on which Codex turn ends count, and
  on nothing else;
- a line's length and depth cost no more than its bytes: the input is also read
  as one line, and as that line nested 4096 arrays deep (serde_json stops at
  128), each read in the time and memory a unit gets.

libFuzzer gives each input 10 s and 2 GB (`-timeout`, `-rss_limit_mb`) and up
to 256 KiB. A cost quadratic in the number of lines (a list of open calls
searched per line) is too small at that size to time out; the readers bound
it by reading a tail of a fixed size (at most 4 MiB).

```powershell
./scripts/fuzz.ps1                                   # each target for 60 s
./scripts/fuzz.ps1 -Target codex_rollout -Seconds 600
./scripts/fuzz.ps1 -Target claude_transcript -Reproduce test-build/fuzz/claude_transcript/artifacts/crash-<sha1>
./scripts/fuzz.ps1 -Minimize                         # shrink each corpus
```

What a run keeps is in `test-build/fuzz/<target>/`: `corpus/`, the inputs that
add coverage, which the next run starts from; `artifacts/`, an input that
crashed (`crash-*`), ran out of time (`timeout-*`) or memory (`oom-*`); and
`run.log`. A run that finds one fails and names the file. `-Reproduce` runs
the target on that one input, with its panic and stack.

`.github/workflows/fuzz.yml` runs each target for five minutes on Mondays and
by hand (`seconds` sets the time), from the seeds alone, and uploads only what
it finds, as `fuzz-<target>-findings`; download it and reproduce from there.

A finding becomes a regression test before it is fixed: the input, cut to what
still fails, goes into a unit test beside the reader it broke, as a literal fed
to the function (`due_in`, `read_transcript`, ...) with what it should read, so
`cargo test` holds it without the fuzzer. When it is a whole transcript worth
keeping, it goes into `fixtures/transcripts/` with its golden too, and so seeds
the corpus as well.

## 3. Live

`src-tauri/src/live.rs` drives background helpers on the real CLIs through the
same path a `luna_spawn` takes: `agents::spawn` with its privilege cap,
`background::start` and `command_for`, the CLI's headless mode, Luna's MCP
server on the hub, and `agents::wait` for the answer. For each provider it
checks that:

- a turn completes with the answer asked for, and on the way the CLI's stream
  gives Luna what it reads off it (the conversation id, the model, the turn
  busy and then over, the transcript on disk);
- a helper with tools calls Luna's MCP server and is authenticated by the
  token in its environment;
- a spawn asking for more privilege than its caller has is refused before any
  CLI starts.

### Accounts

The live tests run only on throwaway accounts in `test-build/accounts`, which
the test build also uses by default:

```powershell
./scripts/test-accounts.ps1 clone -Source <account folder> -Provider anthropic -Name live
./scripts/test-accounts.ps1 clone -Source <account folder> -Provider openai -Name live
./scripts/test-accounts.ps1 list
./scripts/test-accounts.ps1 clean -Name live
```

A clone is a plain copy of the account's settings, skills and plugins, without
its conversations, logs and prompt history, and without its login: sign the
copy in once with the step `clone` prints. A source or destination with a
junction or symlink in it is refused, and so is `clean` through one: a link
there once let test helpers write into a real account.

`-WithCredentials` copies the login too, and warns why that is risky: OAuth
refresh tokens rotate, so whichever of the two folders refreshes first leaves
the other holding a dead token. A live run on the copy can sign the original
out. Prefer signing the copy in on its own.

### Running

```powershell
./scripts/live-tests.ps1 -ClaudeAccount live -CodexAccount live
./scripts/live-tests.ps1 -ClaudeAccount live -NoBuild    # one provider, last build
```

The script checks each account is there, reached through no link and signed
in; builds the Windows test executables (`windows-tests.ps1 -Build`) into
`test-build/live-tests`; and runs only the live tests, one at a time, with
`LUNA_LIVE=1` and `LUNA_LIVE_CLAUDE_ACCOUNT` / `LUNA_LIVE_CODEX_ACCOUNT` set. The
CLI is the test build's managed copy when it has one, else the one on PATH.
Run from `test-build/live-tests`, the executable keeps its own data folder, so
its hub, settings and log (`test-build/live-tests/luna.log`) never mix with a
test build running beside it. From a terminal run as administrator the script
stops at once, and the tests would refuse each start anyway: they check the
token as Luna does (`elevation.rs`), and no test can lift that.

The tests are `#[ignore]`d, so `cargo test` and the gate list them as ignored
and never run them. Even with `--ignored` they do nothing unless `LUNA_LIVE=1`;
with it, an account outside `test-build/accounts/<provider>/`, or reached
through a link, fails the test instead of being used. Each test is time-boxed
(`-TimeoutS`, 240 s by default) and deletes its helper and work folder when it
ends, passed or not. `LUNA_LIVE_CLAUDE_MODEL` / `LUNA_LIVE_CODEX_MODEL` pick a
model; the account's default is used otherwise, at low effort.

The Windows peer-check tests (`the_windows_peer_check_refuses_a_sandboxed_codex`,
`serve_welcomes_a_plain_client_and_refuses_a_sandboxed_one`) need only the
Codex CLI, no account, and are run on their own with `--exact`. Several Codex
sandbox runs in one process can leave its sandbox unable to reach loopback, so
a routine run skips a sandboxed connection that never comes; with
`LUNA_LIVE_STRICT=1` that connection is required and its absence fails the
test, which is how to prove the refusal was exercised.

One more ignored test reaches the machine rather than a CLI:
`power::machine::tests::windows_takes_the_shutdown_request_and_its_abort` asks
Windows for a real shutdown a minute away, through the call shut-down-when-done
makes, and aborts it at once; Windows' own notice shows in between. Run it
alone, by name, from the Windows test executable
(`luna_lib-*.exe windows_takes_the_shutdown --ignored`), and should the abort
fail, `shutdown /a` takes the request back.

### Cost, and why not in CI

Every run spends real quota: a few short turns per provider, on the accounts
named. The answers are a model's, so a failure may be the model rather than
Luna; read the log before changing code, and rerun once. CI never runs this
level: it would need long-lived logins stored as secrets, whose refresh tokens
rotate under it and break the next run (or the account they were copied from);
it would spend quota on every push; and a model's variance has no place in a
gate that must be red only for a real fault.

Run it before a release, after a CLI update, and after a change to how Luna
launches or reads a CLI (`background/`, `pty/start.rs`, `claude/launch.rs`,
`codex/launch.rs`, `mcp.rs`, `hub.rs`).
