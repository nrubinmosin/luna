# Providers: `claude/` and `codex/`

Two CLIs, two independent layers
([ADR-0005](../adr/0005-two-providers-as-independent-layers.md)). Each describes
its CLI in its own terms; `Provider` (`provider.rs`) is a tag and nothing more.
Version-specific observations are in [cli-compatibility.md](../cli-compatibility.md).

## Symmetry

| Concern | Claude Code (`claude/`) | Codex (`codex/`) |
| --- | --- | --- |
| Launch args | `launch.rs` | `launch.rs` |
| Binary source | `cli.rs`: downloads.claude.ai, `latest` + `manifest.json`, bare exe | `cli.rs`: GitHub Releases `openai/codex`, tag `rust-v<ver>`, package tarball + `SHA256SUMS` |
| Defaults | `defaults.rs`: `model`, `effortLevel`, `permissions.defaultMode` | `defaults.rs`: `model`, `model_reasoning_effort`, `approval_policy`, `sandbox_mode` |
| Models | `models.rs`: context windows from the Models API | `models.rs`: catalog with per-level descriptions |
| Limits | `limits.rs`: OAuth usage + CLI cache | `limits.rs`: `wham/usage` + rollout fallback |
| Auth | `oauth.rs`: refresh like the CLI | none: Codex refreshes itself |
| Trust | `trust.rs`: `hasTrustDialogAccepted` in `.claude.json`, written only when the new-chat dialog's box is ticked | `trust.rs`: `[projects.'<path>'] trust_level` when ticked, or a one-run `-c` (a helper, a trusted root) |
| Config writing | JSON (`account_settings/`) | `config.rs`: `toml_edit`, comments kept |
| Hooks unasked | n/a | `hooks.rs`: `--dangerously-bypass-hook-trust` |
| Session on disk | `session/`: registry + transcript | `session/`: rollouts + `session_index.jsonl` |
| UI | `src/features/providers/claude/` | `src/features/providers/codex/` |

Shared mechanics only: the pty (`pty/`), worktrees (`worktree/`), the accounts
folder (`accounts.rs`), CLI downloads (`cli.rs`), the 429 cool-off
(`throttle.rs`), and what Luna applies to a start (`applied.rs`).

## What Luna applies

Luna is a runtime ([ADR-0016](../adr/0016-luna-is-a-runtime.md)): it adds to a
CLI what running it as a session needs, what protects its own boundary, and
what the person chose, and shows each in the chat's "What Luna applied". Each
builder returns the `applied::Item`s it applied beside the arguments. An item
cannot be declared without a class (Runtime, Boundary, UserAction, Workaround)
and a why. Per provider:

| Item | Claude Code | Codex |
| --- | --- | --- |
| Self-update off (Runtime) | `DISABLE_AUTOUPDATER=1` | `-c check_for_update_on_startup=false` per start, never written |
| Guard (UserAction in a chat, Runtime in a helper) | `PreToolUse` in the `--settings` file | `-c hooks.PreToolUse`, only with hooks unasked |
| Reviewer | n/a | `approvals_reviewer="user"` for helpers only |
| Windows sandbox (Workaround) | n/a | `unelevated` unless the account's `config.toml` names one |
| Trust | the CLI's prompt, or the dialog's write | the CLI's screen, the dialog's write, or a helper's `-c` for its run |
| `--skip-git-repo-check` (Runtime) | n/a | a helper outside a Git repository only |
| Core read deny (Boundary) | `permissions.deny` in `--settings` | a profile deny, off Windows (ADR-0014) |
| Library memory | `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD`, narrowed by `claudeMdExcludes` (`memory_excludes`) | n/a: Codex takes no other instructions |

## CLI binaries

`cli.rs` keeps Luna's own copies in `<data>/claude-cli/versions/<ver>/claude.exe`
and `<data>/codex-cli/versions/<ver>/bin/codex.exe`, with `current` naming the
active version folder ([ADR-0013](../adr/0013-own-cli-binaries.md)). Versions are
never overwritten. Nothing else is started, the CLI on `PATH` included: until
the first copy is installed a session is refused as not installed yet, and a
`current` that names a missing, incomplete or unrecorded directory is refused as
damaged (`cli.rs` `launch_in`), never replaced by whatever a bare name finds. Which version
the pass installs is the provider's channel
([ADR-0019](../adr/0019-cli-channels-and-rollback.md), `cli/choice.rs`):
`known-good` (the default) takes the newest version on the list compiled into
the release (`src-tauri/cli-known-good.json`, `cli/known_good.rs`) not marked
bad, fetched by its version; `latest` takes what the CLI's pointer names. Each
is checked every six hours. Claude
Code's `manifest.json` is verified against its detached signature
(`manifest.json.sig`) and Anthropic's release key, pinned in
`claude/manifest_sig.rs`; the exe then against that manifest's sha256 and
size. A new key reaches Luna only with a Luna release. The manifest must name
the version it was fetched for (a signed older manifest served under a newer
version's URLs is a relabel). No version below the floor is downloaded or
taken from a pointer: the highest version that has been current
(`<root>/floor`, raised by `flip_in`; Claude Code's older `signed-floor` still
counts), or the list's `minSafe` on a fresh tree. The pointers are not
signed, so this is what keeps an old release from being replayed; a release
Anthropic pulls back is refused as well, logged, until a newer one comes.
Codex comes as the
package tarball, the only asset with a line in `codex-package_SHA256SUMS`, which
also carries `codex-command-runner.exe` and the other Windows sandbox helpers; it
is unpacked whole into the version folder.

An install is all or nothing:

1. **Staging.** The fetch writes into `<root>/staging/<ver>-<random>/`, on the
   same volume as `versions/`. Whatever an earlier pass left in `staging/` (a
   crash mid-download or mid-unpack) is removed first; a link or junction there,
   or `staging` itself as one, is removed as itself and never gone into.
2. **Validation.** The source says what it put there (`Package`): Claude Code
   the one exe with the signed manifest's sha256 and size; Codex every regular file of
   the tarball at its tar header's size (the release publishes no hash of the
   files inside, so the tarball's sha256 is what pins them). The Codex unpack
   reads the tarball twice through one cap on the decompressed stream
   (`Capped`: the entries' bytes plus a header, padding and a metadata record
   each). The first pass reads the headers raw and refuses before anything is
   written: a PAX record or GNU long name past `MAX_META` (64 KiB; the tar
   reader would take it into memory whole), a PAX `size` override (Python's
   `tarfile`, which writes the package, emits one only past 8 GiB), a link,
   device, pipe or sparse entry, and more than `MAX_ENTRIES` entries or
   `MAX_BYTES` bytes (2,000 and 4 GiB; a package holds about 50 and 600 MB).
   The second unpacks with the PAX records applied, budgeting each entry's
   size as the reader takes it. `validate` checks
   each is a regular file, not a link, at that size and, where given, that
   hash, and that the executable is among them; it records every file's
   sha256, the release's or the one it has as unpacked.
3. **Record.** Every file is flushed to disk (`sync_all`), then
   `luna-install.json` (version, how it was verified, every file with its size
   and hash) is written into the staged folder and flushed, then on
   Unix every folder below is fsynced: a rename that survives a power cut finds
   whole files, not names and sizes over contents still in the cache.
4. **Promotion.** One rename of the staged folder into `versions/` under a name
   nothing has: `<ver>`, or `<ver>~<n>` while an older `<ver>` folder is still
   there. An existing folder, which a running session may hold, is never
   replaced, moved or added to. On Unix `versions/` and `staging/` are fsynced
   after it. Windows cannot flush a folder without flushing the whole volume
   (administrator only): NTFS journals the rename, so after a crash it is whole
   or absent, but a recent one may be undone; then the staged copy is still in
   `staging/` and the next pass sweeps it and fetches again.
5. **Probe.** The installed exe is run offline in a scratch home under
   `staging/` (`cli/probe.rs`): `--version` must name the version, the
   `--help` of each command Luna launches must list every flag Luna passes
   there (the table `COMMANDS`, which the golden launch tests hold to every
   argument list they pin, and fixtures hold to the listed versions' help),
   and Codex must load its configuration with Luna's hook and MCP overrides
   (`features list`). A version that fails is marked bad and swept.
6. **Flip.** The first time, `<root>/recorded` is written. Then `previous` is
   set to what `current` named, the floor rises to the version and a canary
   starts for it (`cli/canary.rs`), all before `current` is
   rewritten through a temp name, flushed before its rename (and its folder
   fsynced on Unix, `secret::write_private_synced`), to the new folder's name:
   a crash or a failed write leaves the old version current under the new
   floor at worst, never the reverse. Each pass first raises the floor to
   what is current (`reconcile_in`), which mends a tree an older order left.
   Then the sweep runs.

A folder counts as installed only by its record, with every file it lists
present at its size; that is checked for each spawn, the hashes only at
install. Without a record a folder is spawned only in a tree with no
`recorded` file, which can only be from before records (whose `current` was
moved only to finished installs); the update pass still fetches its version
again beside it, and the sweep removes the old one once nothing holds it. In a
tree with `recorded`, a folder without its record is refused, whatever is in
it.

The sweep keeps `current`, `previous` and every folder a process of Luna's
runs from: each process the core or the window starts from a managed copy
holds a shared lock on `<root>/in-use/<dir>` until it has exited
(`cli/in_use.rs`, `cli::launch`), and the sweep removes a folder only while
it holds that file's exclusive lock, so no start begins from it meanwhile.
The window sweeps and the core spawns, hence a file lock rather than a count
in memory; the OS lets go of it with a process that dies. The lock files are
never removed: a start that opened one while the sweep held it goes on to lock
that file, which must be the one every later sweep locks too.

Going back: "Use" (`cli_use_version`) makes any installed folder current and
writes `<root>/pinned`, which the pass leaves be until a channel is chosen
again. It holds every file to the sha256 its record kept first; below the
floor every file must have one, and nothing below `minSafe` is taken. "Mark
bad" (`cli_mark_bad`) writes `<root>/bad.json`, which no pass installs or
makes current. The canary counts the first three sessions on a new version
in `<root>/canary.json` (the core counts, the window resets it on a flip,
both under `canary.lock`): two whose CLI exits with an error within half a
minute raise a desktop notice and `cli://canary`, which the status bar and
Settings → CLIs show. Nothing rolls back on its own. In the test build a
file `canary-fail` at the provider's root fails each watched session.

The sweep and the cleanup delete nothing through a link or junction: each
entry is checked with `symlink_metadata` on every component from the CLI root
down to the path (`link_free_below`), and a folder with one on the way goes by
`remove_tree`, which removes a link as itself.

## Defaults

Flags are always set, so `claude/defaults.rs` and `codex/defaults.rs` read the
files themselves and the dialog opens on what they say:

- **Claude Code**, in its order: the account's `settings.json` → the project's
  `.claude/settings.json` → `.claude/settings.local.json` → the machine-wide
  managed file.
- **Codex**: the account's `config.toml`, then the project's
  `.codex/config.toml` (only for a trusted folder). No model there: the first
  model of the catalog (sorted by `priority`; source `codex`); no effort: that
  model's `default_reasoning_level`. Codex always gets `--model`.

Picking another model moves the effort to that model's default unless the effort
came from a file or was picked by hand (`draft.follow`). What Codex actually ran
is read from the rollout's `turn_context` (`modelSeen`). The core normalizes and
supplies stock values (`source: stock`) so the dialog, `luna_session_spawn` and
`luna_session_list` say the same thing.

## Models

- **Claude Code:** context windows from the Models API (`claude/models.rs`); a
  background thread reads the accounts root from Rust settings.
- **Codex:** the catalog (`codex/models.rs`), sorted by priority, with
  per-level descriptions (`effort_hints`): the account's `models_cache.json` →
  another Codex account's → `codex debug models` → `codex debug models
  --bundled`. `models_known` never runs the CLI; `models_for` may.

## Reading a Claude Code session

`claude/session/`: the registry `<config>/sessions/<pid>.json` and the
transcript `projects/<cwd>/<sid>.jsonl`.

- `registry.rs`: registry and meta. `claude -p` writes a `<pid>.json` too, so
  past the pid and the session it was told to resume, the cwd + start-time
  fallback takes only `entrypoint: cli`.
- `transcript.rs`: where a transcript is, and the conversation. A read from a
  cursor (a plain byte offset its callers hold) starts in the state the lines
  before it left: a CLI prompt still open makes what answers it open a turn.
  That state goes with the cursor a read hands back, kept per file and cursor
  with the file's identity (volume and index, or device and inode), its
  creation time and the 4 KiB before the cursor; for any other cursor, or a file
  renamed into place or edited near the cursor since, it is read back from the
  file in 64 KiB chunks, 8 MiB and lines of 1 MiB at most, a prompt further
  back counting as answered. An in-place edit that keeps the length and those
  4 KiB goes unseen: transcripts only grow, and whoever can rewrite one already
  decides what that session reports, so all it can change is its `after_cli`.
- `ending.rs`: context, model, last words, API error, pending tool.
- `titles.rs`: order `custom-title` (a `/rename`) → registry name with
  `nameSource: user` → `ai-title` → first prompt (only while there is no title).
  A registry name with `nameSource: auto`, and the second `ai-title` written
  after a plan is accepted (the plan's handle, repeated by `agent-name`), are not
  titles. Both are rewritten into the tail, so they are read with the context.
  `saved_title` finds a transcript by session id once per launch.
- `reading.rs`: `read_transcript`, all of the above of a transcript's bytes
  alone, by the same per-line functions the file-based readers call.

## Reading a Codex session

`codex/session/`: rollouts `<CODEX_HOME>/sessions/YYYY/MM/DD/rollout-<local
time>-<uuid>.jsonl`, one JSON `{timestamp,type,payload}` per line. Codex has no
live registry.

- **Which file is ours** (`rollouts.rs`): the thread the TUI names in its title
  (`-c tui.terminal_title=["thread-id"]`, OSC 0/2 read off the output). The
  title is cut, leaving 25 hex digits of the uuid, matched against the start of
  the one in the file name; two alike are no answer. That follows `/new`,
  `/clear`, `/fork` and `/resume`, but not `/subagents`, whose title names the
  watched subagent (its rollout says `{"subagent":…}`). Before the title has
  said: for a fresh session the earliest rollout after the spawn whose
  `session_meta` cwd is the folder (or a Codex worktree of it); for a resumed
  one, the uuid. Only `session_meta.source: cli` is guessed (a helper's says
  `exec`). The answer is remembered per chat.
- **Running read** (`scan.rs`), incremental from where the last look stopped:
  `task_started` / `task_complete` → working / resting and the turn count;
  `request_user_input` (holds the turn) and `request_user_input_async` (open
  until the next `UserMessage`) → the question. The turn count goes on over
  every file the process has been on.
- **From the tail** (128 KiB, `read.rs`): `token_count.info` → context
  (`last_token_usage.total_tokens − reasoning_output_tokens` against
  `model_context_window`, as Codex counts it); `turn_context.model` → model;
  `token_count.rate_limits` → limits fallback; `session_meta.cwd` → where a
  `--worktree` session went (`rollout_cwd`). The conversation is `response_item`
  messages with role user / assistant; developer messages and the blocks
  Codex injects in the user's place (`injected`: `# AGENTS.md instructions`,
  `<environment_context>`, `<turn_aborted>` and the rest it recognizes as its
  own) are not. A prompt that opens with a tag of its own is a prompt.
- Approval requests are not in the rollout, so "waiting" for Codex is a turn in
  flight with a screen unchanged for 3 s (Codex draws its spinner continuously),
  or an approval dialog recognized on screen. The rollout appears only with the
  first turn.
- Thread names come from `session_index.jsonl`: Codex writes the first line;
  anything that differs is a `/rename`.
- **Subagents** run inside the session's process and each write a rollout whose
  `session_meta.source` names the parent thread. `children_at_work` reads those
  of a live chat's thread and its descendants (threads begun since the process
  spawned, two weeks at most): one is at work while its last turn has not
  ended.
- `reading.rs`: `read_rollout`, all that the running read and the tail read
  take of a rollout, of its bytes alone, by the same per-line functions.

## The Codex sandbox on Windows

- Without `windows.sandbox` Codex rejects or sends for approval every command
  under `read-only`, `read-only-net` and `workspace-write`. Those modes get
  `-c windows.sandbox="unelevated"` unless the account's `config.toml` names
  a `windows.sandbox` of its own (`config.rs` `windows_sandbox_set`), which
  then stands; a project's does not count (a restricted token per command: no
  administrator, no sandbox accounts; under `workspace-write` Codex adds one ACL
  entry for its sandbox to the folder) and a `PATH` without
  `…\Microsoft\WindowsApps`: the Store PowerShell's app alias fails under the
  restricted token (`CreateProcessAsUserW failed: 5`), and without it Codex
  falls back to an installed PowerShell.
- **`read-only-net`** is Luna's value: `-c default_permissions="luna-<random>"`
  with that profile defined in `-c` (`extends = ":read-only"`,
  `network.enabled = true`) in place of `--sandbox`, nothing written to
  `config.toml`. The name is new on every start, since Codex lays a config's
  rules for that name over these and a known name would let a project's
  `.codex/config.toml` make it writable. `auto-review` cannot use it.
- On Windows the "no network" of `read-only` and `workspace-write` is only proxy
  variables pointing at `127.0.0.1:9` and `GIT_SSH_COMMAND=cmd /c exit 1`: ssh,
  raw sockets and `gh` with those cleared still connect.
- Under the restricted token schannel TLS fails (`SEC_E_NO_CREDENTIALS`) and
  Git's MSYS `sh.exe` / `ssh.exe` die. Luna appends `http.sslBackend=openssl`
  to `GIT_CONFIG_*` (after the user's, unless they chose a backend), so git over
  https works; for ssh set `GIT_SSH` to Windows' own `ssh.exe`.

## Invariants

- Neither layer imports the other's vocabulary.
- A transcript is untrusted input (a prompt-injected agent writes much of
  it): its readers are pure functions of its text, held to golden readings
  of `src-tauri/fixtures/transcripts/` and fuzzed (`src-tauri/fuzz`,
  [testing](../testing.md#fuzzing)). A reader of a file does its I/O, then
  hands the bytes or lines to them.
- Config writes keep the user's comments and key order (`toml_edit`,
  `preserve_order`), and a file that will not parse is an error, never an empty
  document written back ([accounts](accounts.md#writing-cli-files)).
- What runs on an account is known until its process exits (`account_use.rs`).
