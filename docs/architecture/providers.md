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
| Trust | `trust.rs`: `hasTrustDialogAccepted` in `.claude.json` | `trust.rs`: `[projects.'<path>'] trust_level` or a one-run `-c` |
| Config writing | JSON (`account_settings/`) | `config.rs`: `toml_edit`, comments kept |
| Hooks unasked | n/a | `hooks.rs`: `--dangerously-bypass-hook-trust` |
| Session on disk | `session/`: registry + transcript | `session/`: rollouts + `session_index.jsonl` |
| UI | `src/features/providers/claude/` | `src/features/providers/codex/` |

Shared mechanics only: the pty (`pty/`), worktrees (`worktree/`), the accounts
folder (`accounts.rs`), CLI downloads (`cli.rs`), the 429 cool-off
(`throttle.rs`).

## CLI binaries

`cli.rs` keeps Luna's own copies in `<data>/claude-cli/versions/<ver>/claude.exe`
and `<data>/codex-cli/versions/<ver>/bin/codex.exe`, with `current` naming the
active version folder ([ADR-0013](../adr/0013-own-cli-binaries.md)). Versions are
never overwritten; old ones are swept once nobody holds them; `PATH` is the
fallback until a copy exists. Each channel is checked every six hours. Claude
Code's `manifest.json` is verified against its detached signature
(`manifest.json.sig`) and Anthropic's release key, pinned in
`claude/manifest_sig.rs`; the exe then against that manifest's sha256 and
size. A new key reaches Luna only with a Luna release. The manifest must name
the version it was fetched for (a signed older manifest served under a newer
version's URLs is a relabel), and no version below the floor is taken: the
highest one whose signed manifest Luna has accepted (`<root>/signed-floor`), or
`MIN_VERSION` (2.1.319, the one last checked live) on a fresh tree. The
`latest` pointer is not signed, so this is what keeps an old signed release
from being replayed; a release Anthropic itself pulls back is refused as well,
logged, until a newer one comes. Codex comes as the
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
   hash, and that the executable is among them.
3. **Record.** Every file is flushed to disk (`sync_all`), then
   `luna-install.json` (version, how it was verified, every file with its size
   and hash where known) is written into the staged folder and flushed, then on
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
5. **Flip.** The first time, `<root>/recorded` is written. Then `current` is
   rewritten through a temp name, flushed before its rename (and its folder
   fsynced on Unix, `secret::write_private_synced`), to the new folder's name,
   and the sweep runs.

A folder counts as installed only by its record, with every file it lists
present at its size; that is checked for each spawn, the hashes only at
install. Without a record a folder is spawned only in a tree with no
`recorded` file, which can only be from before records (whose `current` was
moved only to finished installs); the update pass still fetches its version
again beside it, and the sweep removes the old one once nothing holds it. In a
tree with `recorded`, a folder without its record is refused, whatever is in
it.

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
supplies stock values (`source: stock`) so the dialog, `luna_spawn` and
`luna_list` say the same thing.

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
- `transcript.rs`: where a transcript is, and the conversation.
- `ending.rs`: context, model, last words, API error, pending tool.
- `titles.rs`: order `custom-title` (a `/rename`) → registry name with
  `nameSource: user` → `ai-title` → first prompt (only while there is no title).
  A registry name with `nameSource: auto`, and the second `ai-title` written
  after a plan is accepted (the plan's handle, repeated by `agent-name`), are not
  titles. Both are rewritten into the tail, so they are read with the context.
  `saved_title` finds a transcript by session id once per launch.

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
  messages with role user / assistant; developer messages, `# AGENTS.md
  instructions` and `<environment_context>` are not.
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

## The Codex sandbox on Windows

- Without `windows.sandbox` Codex rejects or sends for approval every command
  under `read-only`, `read-only-net` and `workspace-write`. Those modes get
  `-c windows.sandbox="unelevated"` (a restricted token per command: no
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
- Config writes keep the user's comments and key order (`toml_edit`,
  `preserve_order`), and a file that will not parse is an error, never an empty
  document written back ([accounts](accounts.md#writing-cli-files)).
- What runs on an account is known until its process exits (`account_use.rs`).
