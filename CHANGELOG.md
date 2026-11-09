# Changelog

All notable changes to Luna are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (pre-1.0: a minor version may change
behavior). Releases before this file existed are summarized per minor version
from the git tags; `git log v<a>..v<b>` has the detail.

## [Unreleased]

## [0.11.0] - 2026-11-09

Settings is a dialog of pages, each CLI follows a known-good or latest channel
you can roll back, a chat can hand itself off to a fresh conversation as its
context fills, and Luna changes nothing in a CLI's behavior without asking and
shows what it applied. Helpers always run in the background, and a library
skill or instructions file can be kept to your own chats or to helpers.
Breaking for agents: Luna's tools are renamed `luna_<resource>_<verb>`
(`luna_spawn` is `luna_session_spawn`, and so on), the old names only answer
with the new list, and visible helpers are gone.

### Breaking (for agents)

- Luna's MCP tools are named for the thing they act on and what they do,
  `luna_<resource>_<verb>`, sixteen of them: `luna_session_list`,
  `luna_session_get`, `luna_session_spawn`, `luna_session_send`,
  `luna_session_answer`, `luna_session_wait`, `luna_session_stop`,
  `luna_session_delete`, `luna_message_list`, `luna_report_get`,
  `luna_account_list`, `luna_account_get`, `luna_run_list`, `luna_guide_get`,
  `luna_notification_send` and `luna_power_set`. The old names (`luna_list`,
  `luna_sessions`, `luna_spawn`, `luna_send`, `luna_answer`, `luna_read`,
  `luna_wait`, `luna_kill`, `luna_delete`, `luna_guide`, `luna_notify`,
  `luna_power`) are gone: called, one answers with the tools there are now. A
  session already running keeps the old list until its CLI reconnects.
- A listing is compact rows, a page at a time, filtered before anything costly
  is read (`luna_session_list`, about 60-80 tokens a row); one session whole is
  `luna_session_get`; accounts are `luna_account_list` and `luna_account_get`;
  past spawns are `luna_run_list`, compact and limited.
- `luna_session_wait` returns the report by default, with how many other
  messages there are and a file of all of them (`reply: "all"` or `"none"`
  for the rest), and the terminal only with `screen: true`. `luna_report_get`
  gives the report alone, starts the helper's autoDelete count over, and with
  `ack: true` collects the answer so no notice follows.
- Arguments say their units: `timeoutSec`, `quietSec`; a read's left-out count
  is `rest`, its tool-call markers `toolCalls`; `luna_session_delete` takes
  `id` or `ids` and answers `already` for one deleted before.
- `luna_guide_get` takes a topic from a fixed list, the tools' names and
  `notices`, `restart`, `helpers`, and with none gives a short index. The
  instructions carry a short recipe for a safe spawn and name the two topics
  to read first; nobody is asked to read the whole guide any more.

### Added

- **Auto-handoff** for a chat with Luna tools, ticked in the new-chat dialog
  or the chat's What Luna applied panel with a threshold (70% of the context
  unless changed). Past it, at a turn's end, Luna asks the chat for a handoff
  file, with the steps and its sections, and a firmer second time near where
  the CLI would compact. On the chat's call of `luna_session_handoff`, offered
  to that chat alone, Luna copies the file, waits for the chat to be at rest
  (a draft holds it), types `/clear` (Claude Code) or `/new` (Codex) and then a
  prompt pointing the new conversation at the copy, with the facts Luna knows:
  helpers and their state, branches, answers owed, text not yet delivered,
  accounts, the chat's settings. Helpers, debts and notices carry on; the chat
  keeps its row, and a divider over its terminal opens the handoff and the
  conversation before. A switch a core restart cuts off is taken up where it
  stood; one that fails leaves the old conversation going, with a notice. See
  docs/user-guide/chats.md and ADR-0020.
- Each skill and instructions file in the library is for your own chats, for
  helpers, or for both (the default), picked in the Skills & instructions
  window's new **For** column. Claude Code sessions of the other kind are
  started without it (`claudeMdExcludes`, `skillOverrides`); Codex skills are
  turned off per run, and since Codex reads its account's `AGENTS.md` in every
  session, instructions for one kind are laid out with a line saying whose
  they are. What Luna applied lists what the audience kept from a session.
  See docs/user-guide/library.md.
- Luna's note to every helper also tells it what it still owes: to stop, before
  its turn ends, the background work it started that nothing needs any more
  (watchers, wait loops, dev servers; not helpers it waits on, nor what its
  task keeps running), keep each caveat beside the claim it limits, and send,
  push or post nothing outward unless its task asks.
- "What Luna applied" (the ⓘ button on a chat's title bar): everything Luna
  added to the chat's CLI at its latest start, grouped as your choice,
  runtime, boundary or workaround, each with its value and why, and what Luna
  wrote into the account (the folder's trust, the library's copies). Tokens
  and Luna's local addresses are never shown. A helper kept to Luna's MCP
  server and with its auto-memory off shows both as boundary items.
- **Watch** in a chat's helper list shows that helper's conversation,
  read-only and live: what its agent asked and what it said, tool calls marked,
  updating every second while it works. A long conversation shows its latest
  part, without very long entries such as a tool's whole output, and says so.
- Each CLI follows a channel, chosen in the new Settings → CLIs: **Known-good**
  (the default) installs the newest version this Luna release lists as checked
  with it (`src-tauri/cli-known-good.json`, which also names the oldest version
  allowed to run); **Latest** installs each new release, as before. A new
  version is run once offline before it is used (its version, every flag Luna
  passes in each command's help, Codex's hooks config) and marked bad if it
  fails. The first three sessions on a new version are watched; two that fail
  soon after starting bring a notice, a status-bar chip and **Roll back** in
  Settings. Nothing rolls back on its own. See docs/user-guide/updates.md and
  ADR-0019.
- Settings → CLIs lists the installed versions of each CLI, tagged current,
  previous, known-good, in use or bad, with **Use** to go back to one (its
  files checked against their checksums first, and held there until you
  follow the channel again) and **Mark bad**.
- `luna_session_get` shows the CLI version a session's process last started
  from (`cliVersion`).

### Changed

- The version before the current CLI is always kept, and so is any version a
  session still runs on: an update no longer sweeps them, which on Linux and
  macOS could delete a running Codex session's folder. Each Codex install now
  records every file's checksum, not only its size.
- The floor below which no CLI version is downloaded rises when a version
  becomes current rather than when it is downloaded, and Codex has one too.
- Settings → Updates shows Luna and the core; the CLIs moved to their own page.
- Settings is a wide dialog with its pages listed on the left (General, Chats,
  Agents & trust, Links, CLIs, Updates) and only the page scrolling, in place of one
  narrow column of ten sections. Every setting is a row with its control on
  the right and one line on what it does; what a change costs (helpers above
  their chat, a deleted worktree's uncommitted work, accounts moved by hand)
  stays in view, and the rest of each explanation folds into a "How … works"
  line. Help text is 12 px at the least and passes WCAG AA in both themes.
  Lists say when they are empty, link exceptions are added from a menu, and a
  browser's long command line can be shown in full or copied. The page list
  and the exception menu work from the keyboard, Tab stays inside the dialog
  and the focus returns to what opened it, and a draft on one page survives a
  look at another. See docs/user-guide/settings.md.
- Confirmation dialogs (deleting a chat, an update, a library change) take
  the focus when they open, keep Tab inside them, leave the window or dialog
  behind them unreachable until they close, and then give the focus back to
  the button that opened them. Enter on a focused Cancel now only cancels.
- Luna is a runtime, not a provider configurator (ADR-0016): it adds to a CLI
  what running it needs and what protects its own boundary, changes nothing
  else without the person's say, and shows what it applied.
  - The guard against endless waits is off in chats unless "Wait guard" is
    ticked in the new-chat dialog; helpers keep it. Chats with Luna's tools no
    longer get it by default.
  - The new-chat dialog asks "Trust this folder in <account>", unticked;
    unticked, nothing is written and the CLI asks itself. A chat an agent
    starts never writes trust (a Codex helper is trusted for its own runs).
    Quick chat opens the dialog for a folder the account has not trusted.
  - Codex: `approvals_reviewer="user"` only for helpers; `windows.sandbox` only
    when the account's `config.toml` names none;
    `check_for_update_on_startup=false` passed on every start and no longer
    written into new accounts (lines already there stay); a helper gets
    `--skip-git-repo-check` only outside a Git repository.
  - Claude Code: with the library's instructions, the other folders added to a
    session (Luna's notes folder, the library's skills folders, the user's own
    `additionalDirectories`) are no longer read as memory.
  - Chats that had the guard because of Luna tools are named in a notice
    once; a chat's ⓘ panel turns the guard on for it.
- **Breaking for agents:** every helper runs in the background. A Codex
  helper's `approval` is `never` (the default) or `auto-review`; one that asks
  a person (`on-request` and the like) is refused, as nobody is there to ask.
  `luna_session_spawn` answers with `id`, `settings`, `turnsEnded`, `sameFolder`, the
  worktree and `accountChoice`; `started`, `screen`, `ask`, `starting` and
  `warnings` are gone with the visible start that filled them.
- **Breaking for agents:** "Open as chat" hands a helper over to you for good.
  It becomes a chat of your own, a top-level row seated in a pane that keeps
  the wait guard until you turn it off: its agent can no longer send to it,
  wait on it, read it as its own, stop or delete it, and is owed nothing more
  by it. The agent hears so once, in a notice that outlives a restart. Text
  still queued for the helper is not delivered, and whoever sent it is told
  where it is. Luna asks before it does this, and while it opens the agent can
  neither stop nor delete it. An opening Luna cannot finish (the window closed
  midway and stays closed) gives the helper back to its agent after 15
  minutes, and the agent is told.
- Deleting a chat lists the helpers its agent runs with what each is doing and
  what its worktree holds. Ticking the box to take them along never stops one
  still at work, or drops a worktree with uncommitted work, by itself: each
  takes a box of its own, and Luna checks both again as it deletes. A helper's
  own Delete waits while it works and keeps such a worktree.
- Chats an earlier Luna ran as visible helpers are your own chats: their rows
  are no longer nested under the chat that spawned them, no agent can stop,
  delete or answer for them, and the guard notice names them too, as they now
  start without it.

### Removed

- **Breaking for agents:** `luna_session_spawn`'s `visible`; a call passing it is
  refused as any unknown argument is. With it go visible helpers, the chats an
  agent started in the window, and what kept them: the window's part in a
  spawn, carrying them on after a core restart, their nesting and reparenting,
  and the rule that one you typed into was never auto-deleted.
- **Breaking for agents:** `luna_session_answer`'s `keys` (raw keystrokes for a
  helper's screen); a helper has none, and a dialog in your own chat is yours.

### Security

- A Codex helper spawned without `connectors` is not started when its
  account's `config.toml` has an MCP server whose name Codex cannot turn off
  for one run (a dotted name such as `team.slack`): such a server stayed on
  while "What Luna applied" said the helper had Luna's server alone. The
  error names the server; rename it, or spawn with `connectors: true`. The
  servers a Codex helper keeps all the same, those of the project's
  `.codex/config.toml` and the plugins on in the account's and project's
  config as Codex merges them, are listed in
  the spawn's result (`mcpLeftOn`) and in "What Luna applied".
- Luna starts no CLI but its own managed copy. Before its first download had
  finished, or with that copy damaged, a session ran whatever `claude` or
  `codex` the system found on PATH, past the checks a managed version passes
  (its signature or checksums, the probe, the known-good list and its
  minimum). Now such a session is refused, saying whether the CLI is not
  installed yet or its copy is damaged; Settings → CLIs installs it again.

### Fixed

- A session starting while the update pass removed an old CLI version could,
  once that version was installed again under the same folder name, run from
  it under a lock no later pass saw, and on macOS and Linux a later pass could
  remove the folder it ran from. The lock files under `in-use/` now stay.
- A release that changed only the CLI policy (`cli-known-good.json`, its
  minimum safe versions) kept the core's fingerprint, so its window attached
  to the old core still running, which went on allowing the old minimum. The
  policy is now part of the fingerprint, and a test holds every file compiled
  into the core to it.
- A Codex helper spawned with `approval: "auto-review"` failed every turn
  after its first: `codex exec resume` refuses `--approve-for-me`, so its
  later turns pass the three settings that flag stands for instead.

## [0.10.2] - 2026-11-08

A security release for the agent tools and the app window: an agent reaches
only its own helpers unless it asks for every session, helpers start without
their account's connectors, the window's page can call only what it uses, and
no CLI runs with administrator rights. It also makes Luna's tools cheaper for
agents to use and fixes several ways a helper's report or a transcript was
misread.

### Security

- An agent can interrupt only its own helpers and theirs: `luna_send` with
  `interrupt: true` reached any session, so one agent could stop another
  chat's helper mid-turn, or a person's chat, and dismiss the question it was
  waiting on.
- Reading, waiting on, sending text to and answering a session that is not
  the agent's own takes `scope: "all"` in the call, and text so sent, an
  answer typed in as a message too, arrives opened `[From session <id> via
  Luna]` rather than as the user's own words.
- `luna_list` and `luna_sessions` list any session's state but give another
  session's question, permission request or task, and others' past spawns
  with their prompts and replies, only with `scope: "all"`.
- Helpers start with Luna's MCP server alone: no MCP servers or connectors of
  their account (claude.ai's Gmail, Slack and other tools that send and write
  in the person's name; Codex's apps and its account's `config.toml`
  servers), unless spawned with `connectors: true`, and Claude Code helpers
  with auto-memory off. A Codex helper keeps the servers of the project's
  `.codex/` and of plugins, which Codex gives no way to turn off for a run.
- Every `luna_*` tool refuses an argument it does not take, or of the wrong
  type, naming those it takes, instead of running on a default: `read_only`
  for `readOnly` spawned a helper that could write.
- The window's page reaches only what it calls, so a script injected into it
  would get no more: each app command is granted to the main window by name
  and refused to any other window or a remote page, plugins are cut to the
  calls the page makes (no menus, trays, new webviews or paths), the bridge to
  the core passes on only the 35 commands the page sends, each on its own lane
  (never the core's `shutdown`), and account and folder paths are checked
  where they come in: an account path must be an account folder Luna lists,
  reached through no link or junction and spelled as on disk, a folder a
  plain absolute path.
- Turning on Settings → Helpers → "Let helpers run with more privilege than
  the chat that spawns them" asks again in a system dialog, which the page's
  own calls cannot answer, and a yes counts only for the state it was asked
  about.
- No CLI runs with administrator rights. Luna started as administrator starts
  its core, and so every session and helper, with the desktop's ordinary
  rights and the user's own environment, not the elevated one's; where there
  are none (no Explorer, another account's credentials, User Account Control
  off) it starts no session and says to restart it normally. The core refuses
  sessions and helpers whenever it runs elevated, and a core an older Luna
  left running elevated is replaced or, with sessions, restarted on the
  banner's button; one whose rights cannot be read counts as elevated. An
  elevated core lets an ordinary window in only to stop it, past the check
  that otherwise turns away a client of lower integrity; a sandboxed one is
  still refused. Root is refused the same way on Linux and macOS, and the
  live tests refuse to run from an elevated terminal.
- The transcript readers, which take what a prompt-injected agent can write,
  are fuzzed: each provider's are pure functions of a file's bytes, held to
  golden readings of the fixtures, with a cargo-fuzz target each
  (`scripts/fuzz.ps1`) that also checks every text read comes out of the
  input, and that a line's length or depth costs no more than its bytes. A
  scheduled workflow runs them weekly.

### Changed

- `luna_list` names each account with its defaults and usage but no longer its
  models, Codex's catalog with every model's levels being most of what a
  listing cost an agent that lists often; `models: true` brings them back.
- A notice says which lines of its answer file are the report (`lines 1-N of
  M`), and `luna_read` with `report: true` gives the report alone, so an agent
  that wants only the report need not read everything else the helper said.
- `luna_guide` takes a `topic`: a word of a section's title gives that section,
  `all` the whole guide, and no topic its opening with the sections to ask
  for. The instruction sessions get says to read it whole before the first
  spawn; a lookup after that costs a section, not the whole manual.
- A notice ready for an agent that another of its helpers is still working for
  waits up to 20 seconds for that one's answer, so helpers finishing within
  seconds of each other wake the agent once, not once each.

### Fixed

- A Codex prompt that opens with a tag of its own, such as `<task>…</task>`,
  is a prompt again: `luna_read` and answers left out every user message
  starting with `<`, and with it the start of the turn that answered it. Only
  the blocks Codex itself puts there (its environment, skills, shell commands,
  interruptions and the like) are left out.
- `luna_answer` refuses answers to a question with no dialog that are one too
  few or too many, rather than send them as if they answered it.
- A command call whose timeout is past any clock (a Claude Code `Bash`,
  `PowerShell` or `Monitor` call, a Codex call's bound) made the time it is due
  wrap round to one long before the call, so a caller waiting on a quiet turn
  heard of it as hung without the call's own deadline holding the notice back;
  it is now due never, as an open call with a long timeout is. Codex token
  counts past 2^64 no longer overflow the total the answering model is found
  by. Both found by fuzzing.
- A Claude Code session's answer to a prompt the CLI put in itself (a
  background task's end) opens a turn of its own for a caller reading the
  conversation on from a cursor too, not only for one reading from the top,
  so the report it leads with is the newest when that prompt and its answer
  land in different reads.
- A transcript line that is not UTF-8 no longer ends the search for a chat's
  opening prompt (the name it goes by until the CLI titles it), in Claude Code
  and Codex alike: it is read as the conversation reads it, a stray byte as
  U+FFFD.
- A helper's notice led with the wrong report when its CLI started a short turn
  of its own after the real one (a background job's end it took up): the short
  line came first and the long report sank among the rest. Such a turn now
  leaves the report to the long message before it, and comes next as the last
  word.
- Helpers followed rules their account's instructions set for the person's own
  chat — one wrote its progress notes in the language set for "the operator".
  The note a helper gets now names the words such instructions use (the
  operator, the user, the person at this machine) and tells it to answer its
  caller in its task's language and form; a Claude Code helper gets the note
  after its first prompt too, not only in its system prompt.

## [0.10.1] - 2026-11-08

A security and reliability release; it also carries the changes of 0.10.0,
which was never published. Claude Code downloads are checked against
Anthropic's signed release, sessions are kept away from the key to Luna's
core, an interrupted CLI install is no longer mistaken for a finished one,
shutting the PC down or restarting it when done works again, and the
updater's signing key is kept out of the release build.

### Security

- Dependencies with published vulnerabilities are updated: rustls 0.23.46 and
  rustls-webpki 0.103.16 (TLS handshake fixes), webpki-roots 1.0.11 (distrusted
  emSign roots removed), zip 9 under the updater (tauri-plugin-updater
  2.14.1) and tokio 1.53.3 (an unsound `AsyncFd` constructor); among the build
  tools, Vite 7.3.7 and source-map-js 1.2.2.
- Every pull request, every change to master and a weekly run check the
  dependencies for advisories, licenses and sources (cargo-deny, `pnpm audit`),
  the history for committed secrets (gitleaks), and the code and workflows with
  CodeQL. `scripts/supply-chain.ps1` runs the same checks locally.
- The updater's signing key no longer reaches the release build. `tauri build`
  runs every dependency's build scripts, proc-macros and frontend tooling, and
  any of them could have read the key it was handed; the installer is now
  built unsigned and signed afterwards in a job of its own, which runs only the
  Tauri CLI pinned by `pnpm-lock.yaml`, without network, and checks the
  signature against the updater's public key before publishing it. The release
  still takes one approval, now given to the signing job. `release.ps1 -Local`
  signs the same way, builds an export of the commit rather than the checkout
  it signs from, and no longer runs the built exe to ask its core fingerprint.
  A key rotation's bridge release is checked against the old public key, given
  as `UPDATER_TRUSTED_PUBKEY`.
- Claude Code's release manifest is verified against its detached OpenPGP
  signature and Anthropic's release key, pinned in Luna, before the exe's
  checksum from it is trusted: whoever serves the download bucket can no
  longer hand Luna an exe of their own with a matching manifest. Codex is
  still checked only against the checksums of its own release.
- The signed manifest must name the version it was fetched for, and no Claude
  Code version below the highest signed one Luna has accepted (or below
  2.1.319 on a fresh install) is installed, so an old signed release cannot be
  replayed under the unsigned `latest` pointer. A signature is held to its
  signed creation and expiration times, and the release key to its expiry
  where the signature gives one.
- Sessions are denied the folder that holds the key to Luna's core, wherever
  their CLI can enforce it: Claude Code's file tools everywhere, and Codex's
  `read-only` and `read-only-net` sandboxes on Linux and macOS. A Claude Code
  session whose deny cannot be written does not start, rather than run without
  it.
- On Windows, where a Codex sandbox can still read that file, the core instead
  refuses the connection: it checks which process is at the other end of each
  loopback connection and turns away one running in a CLI sandbox (a restricted
  or contained token, or a lower integrity level) before it can present the
  key. A normal window is unaffected. Codex's `workspace-write` on Linux and
  macOS remains able to read the file; the threat model (F3) says why.

### Added

- The installer puts `THIRD-PARTY-NOTICES.txt` beside Luna, and each release
  publishes it too: the license texts of every crate and npm package Luna is
  built from, and where their source is; the Rust standard library's own
  inventory; and what the build tools and the installer add: Vite's helpers,
  NSIS and its Tauri plug-in, the WebView2 loader and the Visual C++ runtime.

### Changed

- `luna_power` disarms with the action `cancel`; `off`, its old name, still
  works.
- Release notes are made from this file: a release's notes are its version's
  section, the sections of every version since the last published release, a
  link to the full changes, and the commands and checksums to verify the
  download. The tag's message no longer carries them, and the update chip
  shows the section's opening paragraph.
- Commit messages follow one written-down form (CONTRIBUTING.md, "Commits"):
  a sentence of at most 72 characters saying what the change does, no type
  prefix, a blank line, then why. The commit-msg hook, the gate and CI refuse
  the rest; past commits stay as they are. A refusal says to commit again from
  the message, not to amend, and `amend!` commits are let through to be
  squashed, as `fixup!` ones are.
- Dependabot opens its grouped update PRs monthly instead of weekly, and leaves a
  new Node or Rust major to a hand-made migration.
- Tauri 2.12.5 and its plugins' latest releases; base64, dirs, getrandom and
  toml_edit moved to versions the rest of the tree already carried, which drops
  dirs 6 and toml_edit 0.22 from the build; vt100 0.18 for the screen a helper's
  output is replayed onto.

### Fixed

- A CLI install cut short (Luna killed or the PC off while Codex's package was
  unpacked) could be taken for a finished one on the next check: the executable
  being there was enough, so Codex ran without its sandbox helpers or from a
  truncated file. Both CLIs are now fetched into a staging folder, checked file
  by file against what their release publishes (Claude Code: sha256 and size;
  Codex: the package's sha256, then each file's size), given a completion
  record, and moved into place with one rename under a name no folder has, so a
  version a session is running is never touched. Only a version with its record
  counts as installed; ones installed before this are fetched again once. The
  files, the record and (on Unix) the folders are flushed to disk before the
  version is named current, and once Luna has installed this way a version
  without its record is never run. Sweeping old versions deletes nothing
  through a link or junction below them. Codex's package is unpacked with caps
  on its entries, its bytes as the tar reader writes them, each metadata record
  and the decompressed stream, and a link entry or PAX size override in it is
  refused.
- Shut down and restart when done turn the PC off again. Luna asks Windows
  itself, with the shutdown privilege turned on, instead of running
  `shutdown.exe`, which had been failing without a word; a refusal is logged
  with its Windows error.
- Windows is asked before any session is closed, so a refusal leaves every chat
  running, and a shutdown Windows records as canceled after the chats were
  closed brings them back where they were instead of leaving dead panes. One
  whose fate Windows does not tell leaves the chats closed and says how to bring
  them back, rather than reopening them under a shutdown still on its way.
- Windows' notice during the grace says that open programs are closed by force
  and unsaved work in them is lost, and so does the user guide.
- Shutdown winds Luna down the way restart does, so a chat parked mid-turn is
  not saved as finished.
- A restart that Windows refuses, or that is canceled, no longer leaves Luna set
  to start at the next sign-in.
- The release workflow failed at attesting the npm SBOM (`Unsupported SBOM
  format`): it was written without a serial number, which the attest action
  requires of a CycloneDX file. Both SBOMs now carry one and are checked for it
  as soon as they are made, and the attestations use `actions/attest` in place
  of the deprecated `actions/attest-sbom`.
- The release and test builds failed: `tauri build` refuses a Tauri crate and its
  npm package on different minor versions, and the notification plugin's crate
  had moved to 2.8 while `@tauri-apps/plugin-notification` stayed at 2.7. The
  package is on 2.8 now, and the gate checks every such pair in the lockfiles
  (`pnpm check:tauri`), so a bump to one side alone fails there.

## [0.10.0] - 2026-11-08

A security and engineering release. Agent tools now follow least privilege,
Luna's files are private from the moment they are created, and the build, tests
and releases run in CI. Everyday behavior is otherwise unchanged, apart from the
fixes below.

### Security

- A helper session can never hold more privilege than the session that started
  it, and only a session's own helpers accept its approvals and raw keystrokes.
- Text that a CLI would run as a command (a line starting with `/` or `!`)
  reaches a helper, visible or in the background, only when a full-access
  session sends it to its own helper.
- The app window runs under a strict Content Security Policy instead of none.
- Luna's data folder is made private when Luna is installed and checked again at
  every start, and files holding secrets are private from the moment they are
  created.
- A file in the data folder that someone else planted or replaced with a link
  is moved aside instead of being written to.
- On Windows, the install folder and the folders holding secrets get protected
  permissions of their own, and the installer resets only the permissions it
  owns.
- Secrets come from the operating system's secure random generator and are
  compared in constant time, and each chat's hook URLs carry a secret of their
  own.
- Claude Code receives its token for Luna's tools through the environment
  instead of a config file.
- Luna's local listeners cap connections before authentication, parse requests
  strictly and enforce time limits.
- A worktree path containing a line break can no longer forge a worktree
  record, and a Codex worktree is removed only from a Codex account's worktrees
  folder.

### Added

- A threat model and a vulnerability reporting policy (`SECURITY.md`).
- Documentation in `docs/`: a user guide by topic, architecture pages, decision
  records, notes on how each CLI behaves, and a development guide.
- A contributing guide, issue and pull request templates, and this changelog.

### Changed

- Luna is dual-licensed under MIT OR Apache-2.0.
- Releases are built, signed and attested by GitHub Actions from a version tag,
  behind a maintainer's approval, with SBOMs and checksums published beside the
  installer; `docs/releasing.md` explains how to verify a download.
- CI runs on every push on Linux and Windows: formatting, lints, type checks,
  the frontend's tests and build, and the Rust unit tests on both systems.
- Toolchains and build images are pinned to exact versions, and every build
  step's result is checked.
- The Rust code and the frontend are split into smaller modules along their
  natural seams, with no change in behavior.
- Test coverage grows: frontend tests, golden tests of how both CLIs are
  started, and a live suite against the real CLIs on throwaway accounts.
- The repository is written in US English, which a check enforces, and commit
  messages carry no AI attribution, which a hook and CI enforce.

### Fixed

- Worktrees in a folder written with 8.3 short names, such as a temp or profile
  folder under a long user name, are recognized, so helpers return to their
  worktree and Codex chats keep their history.
- Running Luna as the built-in Windows Administrator no longer finds the data
  folder open to others on every start.
- An elevated Luna no longer moves aside the logs and lock files it created
  itself.
- Enter in the account picker acts on the highlighted account, even after the
  list is reordered or that account is removed.
- A line break typed into an answer or note no longer submits it early and sends
  the rest as a separate prompt.
- Text sent to a background helper after its turn was stopped now starts the
  next turn instead of waiting indefinitely.
- A background helper whose queued messages were all refused no longer starts
  an empty turn, and the sender is told that its text was refused.

### Removed

- `ARCHITECTURE.md`, replaced by the architecture pages in `docs/`.
- The Russian README; the repository's text is in English only.

## [0.9.0] - 2026-11-04

- A folder's skills reach the sessions Luna starts in it, worktrees included.
- A turn that ends on a question to the user shows as waiting.
- 0.9.1–0.9.10 (to 2026-11-06): fixes and refinements.

## [0.8.0] - 2026-11-01

- Helpers survive a core restart, notice loops are broken, read-only Codex with
  network (`read-only-net`), helpers watched without reading them.
- 0.8.2–0.8.3: what a turn leaves running is named and no longer keeps the PC up
  for good; a Linux check.

## [0.7.0] - 2026-10-29

- Skills and instructions of your own, laid out into accounts and folders.
- 0.7.3: background helpers outlive the core; links open in the browser.

## [0.6.0] - 2026-10-15

- The Windows XP look, toned for reading all day; the sidebar slides out from the
  window's edge; Alt+digit peeks a pane.
- 0.6.11: sessions run in a separate core process.
- 0.6.23: background helpers; their questions reach the agent; Codex's sandbox
  works.

## [0.5.0] - 2026-10-15

- The shell redesigned: a task-pane sidebar, a full-width status bar, one icon set.

## [0.4.0] - 2026-09-24

- Agents: a session can spawn, drive and delete other sessions over MCP.
- Keep the PC awake while sessions work, and power off once they are done.

## [0.3.0] - 2026-09-17

- Codex as a second provider.

## [0.2.0] - 2026-08-14

- The installer is released with each version.
- 0.2.16: Luna ships, verifies and updates its own Claude Code binary.

## [0.1.0] - 2026-08-14

- First release: Claude Code sessions in panes, accounts with isolated configs,
  usage limits, worktrees, the updater.

[Unreleased]: https://github.com/nrubinmosin/luna/compare/v0.11.0...HEAD
[0.11.0]: https://github.com/nrubinmosin/luna/compare/v0.10.2...v0.11.0
[0.10.2]: https://github.com/nrubinmosin/luna/compare/v0.10.1...v0.10.2
[0.10.1]: https://github.com/nrubinmosin/luna/compare/v0.10.0...v0.10.1
[0.10.0]: https://github.com/nrubinmosin/luna/compare/v0.9.10...v0.10.0
[0.9.0]: https://github.com/nrubinmosin/luna/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/nrubinmosin/luna/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/nrubinmosin/luna/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/nrubinmosin/luna/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/nrubinmosin/luna/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/nrubinmosin/luna/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/nrubinmosin/luna/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/nrubinmosin/luna/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/nrubinmosin/luna/releases/tag/v0.1.0
