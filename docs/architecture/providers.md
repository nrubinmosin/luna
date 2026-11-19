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
| Defaults | `defaults.rs`: `model`, effort per model (`modelSettings.<model>.effortLevel`, `effortLevel`, the model's catalog default), `permissions.defaultMode` | `defaults.rs`: `model`, `model_reasoning_effort`, `approval_policy`, `sandbox_mode` |
| Models | `models.rs`: the account's own list with per-model levels; `context.rs`: context windows from the Models API | `models.rs`: catalog with per-level descriptions |
| Limits | `limits.rs`: OAuth usage + CLI cache | `limits.rs`: `wham/usage` + rollout fallback |
| Auth | `oauth.rs`: refresh like the CLI | none: Codex refreshes itself |
| Trust | `trust.rs`: `hasTrustDialogAccepted` in `.claude.json`, written only when the new-chat dialog's box is ticked | `trust.rs`: `[projects.'<path>'] trust_level` when ticked, or a one-run `-c` (a helper, a trusted root) |
| Config writing | JSON (`account_settings/` copies a parent's) | `config.rs`: `toml_edit`, comments kept |
| Session on disk | `session/`: registry + transcript | `session/`: rollouts + `session_index.jsonl` |
| UI | `src/features/providers/claude/` | `src/features/providers/codex/` |

Shared mechanics only: the pty (`pty/`), worktrees (`worktree/`), the accounts
folder (`accounts.rs`), CLI downloads (`cli.rs`), the 429 cool-off
(`throttle.rs`), and what Luna applies to a start (`applied.rs`).

## What Luna applies

Luna is a runtime ([ADR-0016](../adr/0016-luna-is-a-runtime.md)): it adds to a
CLI what running it as a session needs, what protects its own boundary, and
what the person chose. Each
builder returns the `applied::Item`s it applied beside the arguments. An item
cannot be declared without a class (Runtime, Boundary, UserAction, Workaround)
and a why. Per provider:

| Item | Claude Code | Codex |
| --- | --- | --- |
| Self-update off (Runtime) | `DISABLE_AUTOUPDATER=1` | `-c check_for_update_on_startup=false` per start, never written |
| Renderer and TUI (Runtime) | `tui: "fullscreen"` in the `--settings` file and `CLAUDE_CODE_NO_FLICKER=1`, chats and logins | `-c tui.notifications=false`, `-c tui.copy_on_select="never"`, chats only |
| Reviewer | n/a | `approvals_reviewer="user"` for helpers only |
| Windows sandbox (Workaround) | n/a | `unelevated` unless the account's `config.toml` names one |
| Trust | the CLI's prompt, or the dialog's write | the CLI's screen, the dialog's write, or a helper's `-c` for its run |
| `--skip-git-repo-check` (Runtime) | n/a | a helper outside a Git repository only |
| Core read deny (Boundary) | `permissions.deny` in `--settings` | a profile deny, off Windows (ADR-0014) |
| Library memory | `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD`, narrowed by `claudeMdExcludes` (`memory_excludes`) | n/a: Codex takes no other instructions |

## CLI binaries

`cli.rs` keeps Luna's own copies in `<data>/claude-cli/versions/<ver>/claude.exe`
and `<data>/codex-cli/versions/<ver>/bin/codex.exe`, with the record (`state.json`, below) naming the
active version folder ([ADR-0013](../adr/0013-own-cli-binaries.md)). Versions are
never overwritten. Nothing else is started, the CLI on `PATH` included: until
the first copy is installed a session is refused as not installed yet, and a
record that does not read, or names a missing or incomplete directory, is refused as
damaged (`cli.rs` `launch_in`), never replaced by whatever a bare name finds. Which version
the pass installs ([ADR-0030](../adr/0030-scope-of-0-20.md), `cli/choice.rs`
`plan`): the one the CLI's release pointer names, asked as the window starts and every six hours while it runs,
unless it is below the floor, older than the version current here (no step
down: the pointer going back is logged and left), or turned away (the record,
below). Claude
Code's `manifest.json` is verified against its detached signature
(`manifest.json.sig`) and Anthropic's release key, pinned in
`claude/manifest_sig.rs`; the exe then against that manifest's sha256 and
size. A new key reaches Luna only with a Luna release. The manifest must name
the version it was fetched for (a signed older manifest served under a newer
version's URLs is a relabel). No version below the floor is downloaded, made
current, rolled back to or started: one per provider, compiled into the Luna
release (`cli.rs` `floor`), raised only by editing it there when an older
version is found unsafe; nothing on the machine moves it. The pointers are not
signed, so the floor and the no-step-down rule are what keep an old release
from being replayed; a release Anthropic pulls back is not followed down,
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
   (`features list`). The version holds an in-use lock of its own from
   before its promotion's rename to its flip, so no sweep takes it
   meanwhile. An answer that is wrong (`probe::Failed::Wrong`) turns the
   version away and it is swept, not fetched or probed again while the
   pointer names it; a probe that could not ask (`Failed::Unasked`: the exe
   not run or gone, a run past its 30 s, as a scanner holding a fresh exe can
   make it, output that would not read or ran past 1 MiB, elevation, Luna's
   scratch home not to be had) turns nothing away, and the next pass asks
   again of the same install. An install already in `versions/` (a pass cut
   short before its flip) is held to every sha256 its record kept before it
   is taken up, else fetched afresh beside it.
6. **Flip.** One write of the record (below, `flip_in`): the version
   current, a fresh canary for it, and what ran until now the one to go back
   to, unless that was on its way out itself (turned away, or its canary
   tripped), when the one before it stays. Refused for a version the core
   turned away since the pass chose it. Then the sweep runs.

**The record.** Everything about a provider's tree that changes together is
one file, `<root>/state.json` (`cli/state.rs`): the folder current and the
one to go back to (each with its version), the versions turned away with
who turned them away, and the canary. Every change of it (a flip, a
canary's count, a trip, a rollback, a mark made or dropped) reads it, changes
it and writes it back whole under the file lock `<root>/state.lock`, through
a temp file in the same folder, flushed and renamed over the old
(`secret::write_private_synced`); a reader that only looks (a launch,
Settings) reads it without the lock. A crash leaves it as it was or as it was
to be, never between, so the window's pass and the core's canary and
rollback, two processes, never leave each other a half-made state. What that
rests on: the rename replaces the file atomically where it is let happen. On
Windows a scanner or another program holding the file without delete sharing
can refuse it; the change then fails as a whole and the next one makes it.
Unix flushes the folder after the rename; Windows cannot flush a folder, so
there a power cut may undo the most recent change (it leaves the old record
or the new one, not a torn one; no claim is made beyond that). The lock is
held for the read and the write only, never across a download, a probe, a
hash or a sweep.

**A record lost.** A tree with no record to go by (`state::lost_in`:
`state.json` missing with no 0.19 files to migrate, or there and not
parsing) is rebuilt from what is on disk before anything goes by it
(`cli.rs` `recover_in`, at a launch, at the pass's start and at the core's):
the newest installed version at or above the floor whose every file still
matches the sha256 its completion record kept becomes current, the next such
version the one to go back to. The hashing is done outside the lock, and the
record is written only from what verified and only while it is still lost.
Where nothing verifies nothing is written: a launch refuses as before, and a
pass that reaches the pointer installs as on a fresh tree, its flip writing
the record. Its marks and its canary are lost with it: a release rolled back
from or refused by the probe may be tried once more, and a canary that had
tripped is not taken up.

A folder counts as installed only by its completion record, with every file
it lists present at its size; that is checked for each spawn, the hashes only
at install and before a folder is taken up again. A folder from before
completion records (one the migration found current in a 0.19 tree with no
`recorded` file) is run while its exe is there, marked so in the record; the
update pass fetches its version again beside it. In a tree that had
`recorded`, a folder without its completion record is not taken in.

The sweep (`prune_in`) removes every version folder but those the record
names and those an in-use lock covers. Each process the core or the window
starts from a managed copy holds a shared lock on `<root>/in-use/<dir>` until
it has exited (`cli/in_use.rs`, `cli::launch`), as does a promotion from
before its rename to its flip, and the sweep removes a folder only while it
holds that file's exclusive lock, so no start begins from it meanwhile. The
window sweeps and the core spawns, hence a file lock rather than a count in
memory; the OS lets go of it with a process that dies. The lock files are
never removed: a start that opened one while the sweep held it goes on to
lock that file, which must be the one every later sweep locks too. Each
folder is judged only once the sweep holds it, by the record read afresh
under its lock then, so a flip that named a folder after the sweep began
(and let go of its promotion's hold) is seen and the folder spared (tested
with a hook between the two). The folder is then removed outside the
record's lock: while the sweep holds it no write can come to name it, as a
flip names only a folder it holds and a rollback only the one the record
already names. A tree with no record to go by is not swept at all.

**Rollback.** The canary counts the first three sessions on a new version in
the record (the core counts; the window's flip starts it afresh). Two whose
CLI exits with an error within half a minute trip it (`warned`, one write),
and the core rolls back on its own, on a thread of its own so the reaper that
saw the exit does not wait on the hashing (`cli.rs` `roll_back_in`): only
while that version is still current, only to the folder the record keeps to
go back to, of another version, not below the floor and not turned away
itself, once every file matches the sha256 its record kept (a record without
one per file is refused), checked before the write and outside the lock.
Then one write, which holds to the record again (the same folders, the one
to go back to still not turned away): the version before current, the
failed one turned away, nothing kept to go back to, a fresh canary. The
failed folder is swept once nothing runs from it, and a version gone back to
whose own first sessions fail stays, with nothing to go back to (the
failures were then not the version's). A rollback the tree refuses (nothing
to go back to, nothing Luna can vouch for) is noted in the canary's
`outcome` and not tried again; one whose record would not read or write is
not noted, and is tried again. The trip and the rollback are the one change
that takes two writes: a core gone between them, or whose rollback write
failed, is followed by one that makes the rollback before any session starts
(`finish_rollbacks`, from `coreproc.rs`), read from the record (a current
version whose canary tripped with no `outcome`); that core also gives back
the watches the last one held in memory for sessions it never judged
(`canary::release_orphans`). The log, a desktop notice and `cli://canary` say
what came of it. Tested: a flip and a rollback cut before their write and
between their write and their sweep, the one-before turned away between the
rollback's hash and its write, a failed rollback write taken up again, and
the sweep's re-read with a hook between its listing and its hold; a pass and
a rollback on two threads are run too, which shows only that the outcomes it
happens on are sound.

A version turned away is not taken while the pointer names it: each pass
that reads the pointer drops every mark of another version, so a release is
tried once and taken again only if the pointer moves away and comes back. A
watched session past that half minute with no failure (ended well, stopped,
or still running when a timer of the core's looks) adds to `settled`; one
whose process never came up (its spawn failed: `session_running` never
called) is not counted either way. Three settled with no failure is a
version through its first sessions, which the log says once. A core that went
away meanwhile leaves those still inside their half minute unsettled, so the
count stays short rather than claim more than was seen. In the test build a
file `canary-fail` at the provider's root fails each watched session.

**Migration.** A tree without `state.json` but with Luna 0.19's files is read
as they say (`current` and `previous`, a folder's name each; `recorded`;
`bad.json`, plain-string marks by hand or by its probe, kept as `earlier`)
and taken into the record by the first change, which then removes them, with
`canary.json`, `canary.lock` and 0.19's `channel`, `pinned`, `good.json`,
`marks.lock`, `floor` and `signed-floor`. No canary is carried over: nothing
rolls back from a version a person kept (its `dismissed`), and the version
current is taken as through its watch. 0.19 never rolled back on its own, so
no rollback of then is found half made.

An exit that says nothing of the version does not count and gives its watch
to the next session: one whose last words name a cause outside the CLI
(`cause_outside`, over `LastWords`: what the session said, a pane's last
screen or a helper's last 4 KB of stderr, and the failure it gave, a pane's
last line that is not blank or what a helper's own stream said the turn
failed on, `background/run.rs`), and one Luna stopped within the half minute
(`session_stopped`: a chat deleted or restarted, a login window closed, a
helper's turn stopped, Luna quitting); one stopped later just stops being
watched. An exit with no code (a signal's end on Unix, a status that would
not read) counts as one with an error. The causes are known by the CLI's own
words, colors taken out: a line of what it said that begins as Claude Code's
"Error: Can only use --worktree in a git repository, but" or `codex exec`'s
"Error: git command failed: fatal: not a git repository" (`NOT_THE_VERSION`;
those words anywhere else on the screen, a prompt or a folder's name, name
nothing), or the failure holding, in any case, words either CLI ends a
session with for want of a sign-in, of usage left, or of the network or the
API (`OUTSIDE_ANYWHERE`: "please run /login", "not logged in", "invalid api
key", "401 unauthorized", "sign in again", "you've hit your session limit",
"you've hit your usage limit", "quota exceeded", "connection error", "api
error: 5", "overloaded", "stream disconnected", "exceeded retry limit" and
the like, as [cli-compatibility](../cli-compatibility.md) lists them). Only
the failure, so such words above it (a reconnect got past, a prompt) do not
hide a crash. Residual: a pane's failure is its last line, so a CLI that
prints a footer after its error has that error counted (towards a rollback),
and one whose last line someone typed with such words has a real failure
uncounted. A start `pty/folder.rs` refuses never runs, so it is never
watched. Counting only exits after the CLI's own startup was weighed and
left: a version that dies as it starts is what the canary is for, and a pane
gives no sign of a finished startup both CLIs share.

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
  `.codex/config.toml` layers from its root down to the folder, found as
  Codex finds them (only for a trusted folder; `codex/config.rs`
  `project_layers`). No model there: the first model of the catalog (sorted
  by `priority`; source `codex`); no effort: that model's
  `default_reasoning_level`. Codex always gets `--model`. The approval and
  the sandbox fail closed: a layer that will not read or parse, or a value
  Luna does not know (`untrusted` among them, which 0.169.0 refuses), gives
  `on-request` and `read-only` (source `closed`), a value left unsaid beside
  one unknown too; `on-failure` reads as `on-request`; a `{ granular = … }`
  table is `granular`, and a chat in it is started with the table as
  `-c approval_policy=…` (`granular_policy`), never `--ask-for-approval`,
  which takes `on-request` and `never` only. `launch_mode` puts what the
  window asks through the same reading.

Picking another Codex model moves the effort to that model's default unless the
effort came from a file or was picked by hand (`draft.follow`). Picking another
Claude model moves it to that model's default only where the model does not
take the level shown, and then whoever chose it (`draft.force`); a model that
takes no effort shows no effort buttons and runs without `--effort`. What Codex actually ran
is read from the rollout's `turn_context` (`modelSeen`). The core normalizes and
supplies stock values (`source: stock`) so the dialog, `luna_session_spawn` and
`luna_session_list` say the same thing.

## Models

- **Claude Code:** the models the account may pick (`claude/models.rs`),
  never another account's (plan and organization decide the list): the
  account's `cache/model-catalog/*-cc.json` while its `staleAt` has not passed,
  unless the account's `settings.json` or the machine's managed settings say
  `availableModels`, `deniedModels` or `modelPicker` (Claude Code applies
  those to its own answer, not to the file, by rules of its own)
  → what the CLI answered to the SDK's `initialize` control request, asked on
  the account in the background (`claude -p --input-format stream-json
  --output-format stream-json --verbose --safe-mode --strict-mcp-config
  --no-session-persistence` in an empty folder of Luna's, `claude-models-cwd`:
  no hook, plugin, skill, CLAUDE.md, MCP server or project setting of the
  account's loads; one `control_request` line: no prompt, no turn). Once it
  has answered, or not in 20 s, its stdin is closed and it gets 10 s to exit
  by itself (`EXIT_GRACE`), on a thread of its own while the answer is used:
  the CLI may be renewing the account's token then, and saves the rotated
  tokens after its answer, so a kill at the answer could leave the account
  signed out (docs/cli-compatibility.md, 2.1.325). Only past that is it taken
  down. It runs in a Job Object of its own on Windows (not one ended by
  Luna's exit) and a process group of its own on Unix, and what is left in
  there once it has exited, or at the deadline, is taken down: a hook it
  started (managed hooks run even in safe mode), which would otherwise run
  on and hold its stdout open. On Windows nothing in the job can break away,
  so the sweep ends whatever holds the stdout. On Unix the sweep reaches the
  group and, while the CLI still runs, its descendants: one that made a
  session of its own and was orphaned before the sweep escapes it, and may
  keep the ask's stdout reader blocked until Luna exits (a thread, which
  nothing waits on). Luna's own exit (the end of the event loop: Quit, a
  tray Restart; an update's install on Windows; Windows' Restart Manager
  closing it) starts no more asks, has one still waiting for its answer stop
  waiting and take its 10 s at once, and waits for every ask's sweep, up to
  12 s (`wait_for_asks`). Before an install the asks are refused only while
  that wait lasts (`wait_for_asks_before_install`): the install may never
  come (a page reloaded meanwhile, or one that fails), and nothing would
  then let them start again; an ask started between the wait's end and the
  install's end of the process is not waited for. A Windows
  shutdown or sign-out does not wait: the OS ends the CLI with the rest. A
  launch of Luna during that wait is handed to the instance on its way out
  (the single-instance check) and comes to nothing; a tray Restart avoids it
  by having the new window wait for the old one's exit (`--after`); a launch
  by hand then has to be made again once Luna is gone (a known limit). It holds the account's own lock in
  `claude-config-locks/` until it is gone, since the CLI rewrites
  `.claude.json` as it starts and exits; Luna's writes of that file (a trust
  mark, a chat's trust ticked in the new-chat dialog, a recommended key)
  take the same lock and wait up to 35 s (`ASK_WAIT`), so a chat started
  right after an ask can wait for the CLI's exit: about 1 s after the answer
  as measured, at most 30 s after the ask began (20 s without an answer, then
  the 10 s grace), and
  nothing else, settings.json or another account, waits on it
  and kept per account in `claude-models.json` in the data folder for six
  hours → the four aliases. A stale catalog or answer is returned at once and
  asked again behind; an answer newer than the catalog wins over it; a
  signed-out answer (`account.tokenSource: none`; signed in, `account` has a
  `subscriptionType` and no `tokenSource`) is used but not kept. An answer is
  kept with a hash of what those settings said of the restricting keys when
  it was given, and one given under other restrictions is dropped, current
  or stale, so a list narrowed or widened since is asked for again. Each
  format is read strictly: a surprise skips that source, logged once per
  account. `models_known` never runs the CLI (spawn checks,
  `luna_session_list`, `claude_defaults`, and `claude_models` with
  `ask: false`, which every start's defaults read, a quick chat's included);
  `models_for` (`claude_models`) may.
  What `--model` gets (`slug`): a family's main model by its alias, so the
  chat follows the family to its next version as the CLI does; any other
  model by its full id. A catalog model is its family's main model when it is
  in the `main` section and the alias its `short_name` spells is one
  `provider_alias_targets.<alias>.default` points at it; in an `initialize`
  answer, when its `value` is an alias. The dialog keeps a family by its label
  (`Opus`) and a model by its id (`claude-opus-4-7`), `More…` holding the
  latter. Each model's levels are its own, plus `ultracode`, which Claude Code
  runs at any level; the aliases offer every level. Picking a model that does
  not take the level shown moves it to that model's default, a hand-picked
  level too; a model that takes none (Haiku 4.5) runs with no `--effort`, in
  a pane and as a helper. A helper's effort is checked against the model it
  will run on, the account's or project's default model included.
- **Claude Code context windows** from the Models API (`claude/context.rs`); a
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
  `saved_title` finds a transcript by session id once per launch; the window's
  command of that name takes its account path as an `AccountPath`
  (`boundary.rs`), for Codex's thread titles too.
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
  every file the process has been on. Each chat's tracker, and its record of
  its subagents' rollouts, has a lock of its own, held over the read: one
  chat's read never holds up another's look.
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
- A thread's rollout once found (`rollout_of_thread`, `thread_file`), and a
  thread looked for in vain, are kept, up to 1,024 of each before the lookup
  starts over.
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
