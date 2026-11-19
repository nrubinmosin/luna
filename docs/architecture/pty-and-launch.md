# pty and launch

How a session is started, what it is started with, and how its terminal is run.

## Responsibility

- Lay out each CLI's command line and environment from what the start has
  settled: `claude/launch.rs` and `codex/launch.rs` are pure (they read no
  environment, disk or Luna state, and log, write, register or mint nothing).
- Settle a start under the chat's spawn gate (`pty/start.rs`; a helper's turn in
  `background/command.rs` through the same functions): the MCP token and
  registration, the settings, MCP and instructions files, and the
  library pass, what the sandbox takes from this machine; then spawn in
  a pty (`pty/spawn.rs`).
- Keep the scrollback, answer terminal queries, read the screen, and write
  input the way each CLI needs it.
- Say what each start applied: the builders return `applied::Applied` items
  beside the arguments ([ADR-0016](../adr/0016-luna-is-a-runtime.md)), `Launch`
  carries them, and `pty::spawn` hands them to the registry
  (`agents::set_applied`) once the process runs; a helper's turn does the same
  in `background/run.rs`. The goldens check that every flag, `-c` key and
  variable of a start is named by an item (`Redact::assert_shown`). No window
  or core command shows them: 0.20 cut "What Luna applied" and its ledger of
  account writes.
- Finish a process's spec in one place, `pty::finish_spec`: `TERM` for a
  pane's CLI, the folder it starts in, and the variables it goes without
  (`removed_env`), each as applied. `Launch::finished` runs it before the
  golden capture, and `command_for` at the end of a helper's turn.

## Key modules

| Path | What it holds |
| --- | --- |
| `pty/mod.rs` | the session map, `PtyManager`, `manager()` (the one per process) |
| `pty/start.rs` | `ensure_claude_session` / `ensure_codex_session`: what a start settles under the chat's spawn gate, then the command line |
| `pty/spawn.rs` | the shared spawn, scrollback (`Scrollback`), `pty://output` / `pty://exit`, cursor queries answered |
| `pty/control.rs` | write, resize, keys, kill, quitting the way a person would, delete, orphans |
| `pty/screen.rs` | the screen drawn (`vt100`) and kept per session (`Drawn`), composer and draft, terminal modes and title, Codex key events on Windows; `SessionMeta` |
| `pty/probe.rs` | `ActivityProbe::working_dir()`, `sessions_meta` (every chat's look, one call a tick) by provider, tails, sweep of hook and MCP files |
| `stamped.rs` | what is read off a file, kept while its length and change time stand (`ByStamp`) |
| `claude/launch.rs` | Claude Code's arguments and environment (`chat_args`), the settings file's hooks and `tui`, the helper's system prompt line and settings (`claude_helper_args`, `HelperSettings`, used by `background/command.rs`) |
| `codex/launch.rs` | Codex's arguments and environment, approval and sandbox overrides, `read-only-net`, `TUI_SETTINGS`, Luna's MCP server |
| `applied.rs` | the items Luna may apply, each with its class and why |
| `golden.rs` | placeholders that make launch golden tests stable |

## Command lines

Flags are always set (so the CLI's own settings would be overridden): the
defaults readers fill the dialog from the files first, so the flags match them
unless changed by hand ([providers](providers.md#defaults)).

**Claude Code:**

```
claude --model <alias|id> --effort <a level of the model's, or ultracode>
       --permission-mode <mode> [--resume <uuid>] [--worktree[=<name>]]
       --settings <data>/hooks/<chat>.json [--mcp-config …] [--add-dir …]
```

`cwd` is the chat's folder; `CLAUDE_CONFIG_DIR` is the account folder;
`DISABLE_AUTOUPDATER=1`; `CLAUDE_CODE_NO_FLICKER=1`, which asks for the
fullscreen renderer above the CLI's own `fullscreenAutoDisabled` (the settings
file's `tui: "fullscreen"` says the same, below that); every CLI process Luna
starts first loses it from what it inherits (`pty/spawn.rs` `removed_env`), so
a core started from a fullscreen chat's shell does not hand it to helpers, and
only a Claude Code chat or login is given it again. `<chat>` here and in every other file or folder named
for a session (`mcp/`, `hooks/<chat>/`, `media/`, a note's name)
is the id as `paths::id_file_name` spells it: Luna's own ids as they are, any
other byte as `.` and two hex digits, so the login pane's
`login:<provider>:<name>` could never make a name Windows refuses. A login
session (its folder the account folder) is no chat and writes no file at all:
no hooks, tools, notes or library, and its one setting, the
core-folder deny and `tui`, goes inline as `--settings {"permissions":{…},"tui":"fullscreen"}`. A worktree chat whose worktree is known resumes from
the project folder as `--worktree <name> --resume <uuid>` rather than starting
inside the worktree, because Claude Code removes a clean worktree on exit and,
given the name, makes it again with the same name and branch. `--resume` is
passed only when the transcript exists; otherwise the chat starts afresh.

**Codex:**

```
codex [resume <uuid>] -C <cwd> --no-daemon [--worktree] --model <m>
      -c model_reasoning_effort="<e>" -a <on-request|never> -s <sandbox> [-c …]
      -c check_for_update_on_startup=false -c tui.notifications=false
      -c tui.copy_on_select="never" -c tui.terminal_title=["thread-id"]
```

`CODEX_HOME` is the account folder. The three `tui`/update `-c`s are what a
Codex TUI is told at every start instead of a line in the account's
`config.toml` (keys seen in 0.169.0); a helper's `exec` has no TUI and takes
the update check alone. A helper adds `-c approvals_reviewer="user"`
and its folder's trust for the run; a person's chat gets neither, so its
account's or project's reviewer stands and an untrusted folder shows Codex's
own trust screen. `never` + `danger-full-access` becomes
`--dangerously-bypass-approvals-and-sandbox`; `auto-review` is
`--approve-for-me` in place of `-a`/`-s`, in `workspace-write`.
`--no-daemon` keeps the session in this process and pty rather than in Codex's
shared server (which the `-c` overrides rule out anyway). Codex refuses
`--worktree` on resume, so a worktree chat resumes as `resume <uuid> -C
<worktree>`; a worktree gone fails the start. Login is the same command as
`codex login` in the account folder.

The core does not start a folder that does not exist: portable-pty would
silently substitute the user's profile. Before anything of a start is made
(files, token, registration, the library pass), `pty/folder.rs` `folder_check`
refuses only where Luna can be sure the CLI could not start: a folder that is
not there, and a start that passes `--worktree` (Claude Code: any, by name too;
Codex: a new session) in a folder with no `.git` in it or above it, which both
CLIs exit on at once (Claude Code 2.1.319, Codex 0.167.0). Claude Code also makes
a worktree outside git through a `WorktreeCreate` hook; Luna does not look for
one and offers no worktree there. The refusal is the start's error, which the pane
shows as `[failed to start session: …]`. The same check answers the new-chat
dialog (`chat_folder_check`, off the window's thread, asked for every folder):
its refusal disables Create, and its `inGit` decides whether the Git worktree
box is offered at all, so a folder outside git is never asked for one;
`createChat` asks again before it writes trust. It refuses a helper's spawn
(`agents/spawn`) in a folder that is not there. A login runs in its account
folder and is not checked.

The box is a flag for the CLI and nothing more: Luna does not fetch origin
first, does not make the worktree, does not look at it afterwards and does not
remove it, with its chat or otherwise. Claude Code 2.1.325 takes
`-w, --worktree [name]`, Codex 0.169.0 `--worktree`; both are in the box's
tooltip, which says Luna does not remove the result.

## Golden tests

The goldens run the real start, not the builders alone.

- **In a pane** (`pty/start.rs`, `golden_claude_*`, `golden_codex_*`): the
  start runs through `ensure_*` and `pty::spawn` under the spawn gate, side
  effects and all, and `pty::spawn::capture` (test builds only) takes the
  `Launch` in place of creating the process. A start runs in a project folder
  of its test's own (`golden::project`: in the temp folder, with a `.git` of
  its own, removed as the test's thread ends), put as `/golden/project` in
  what is compared. Each test compares the whole
  argument list, the environment, the cwd, the resume decision and the session's
  description, and reads back the settings, MCP and instructions files written
  for it, and what it applied (`Started::applied`, placeholders in). Cases:
  chats, permission modes, resume only with a transcript, worktrees by name,
  login, no guard or hook-trust bypass in any start, chats with tools (Luna's
  MCP, role; none of a helper's items), a `windows.sandbox` of the user's kept, Codex
  approval / sandbox pairs, library folders (skills, instructions, plugins,
  hidden skills, the other added folders' memory left out).
- **Headless** (`background/command.rs`, `golden_headless_*`): a helper turn's
  argv, folder and environment, and the settings and MCP files where the CLI takes them, for both CLIs: first turn,
  resume and worktree, sandboxes and the reviewer, tools, library folders,
  `--skip-git-repo-check` only outside a repository. What
  goes on stdin (the prompt, Luna's note, slash commands kept out of a
  command's arguments) has tests of its own there.
- **Redaction is strict** (`golden.rs` `Redact`): only what changes per run is
  replaced, and each placeholder is first checked to be what it stands for. The
  hub's URLs, the data-dir files and the notes folder become `<hook-url>`,
  `<mcp-url>`, `<settings-file>`, `<mcp-file>`,
  `<instructions-dir>`, `<notes-dir>`; `LUNA_MCP_TOKEN` becomes `<token>` only
  once it is the token the registry knows this session by; a sandbox's `PATH`
  becomes `<sandbox-path>` only if it is this process's minus the Store's app
  aliases; the `GIT_CONFIG_*` TLS entries (Windows) become one
  `<git-ssl-backend>` only if they are exactly this process's plus the backend;
  a `read-only-net` profile's random name becomes `<profile>`. What was applied
  is checked to hold neither the session's token nor a hub URL
  (`Redact::applied`), and to name every flag and its value, every `-c` key,
  every variable set and the cwd of the process as it runs
  (`Redact::assert_shown`), so the two cannot drift.

Any change to a launch path shows up as a reviewed diff of the expected values.

## Hooks

Every Claude Code session but a login starts with a `--settings` file adding hooks that post
to the hub with `curl.exe` (elsewhere `curl`). Hooks print nothing: Claude Code
adds the stdout of a hook exiting 0 to the context. The one settings file also
carries the library's `skillOverrides` and `claudeMdExcludes`
(`claude_settings_file`), since only one `--settings` flag is taken. With the
library's instructions, `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1` would
have Claude Code read the memory of every added folder, so `claudeMdExcludes`
also names the memory files of the others: the notes folder, the library's
skills folders, the user's `permissions.additionalDirectories`
(`claude/launch.rs` `memory_excludes`), each unless it lies above or below the
folder worked in.

A Claude Code helper's settings file holds no hooks (its stream says it all);
Codex gets none of Luna's, and Luna never passes `--dangerously-bypass-hook-trust`,
so Codex asks for the trust of the hooks a project brings as it would started by
hand. The wait guard Luna had until 0.19 (a PreToolUse hook refusing `tail -f`
without a deadline) is gone with the bypass it needed on Codex.

## Scrollback and terminal state

- The output buffer (2 MB cap) lives in the core, so moving a chat between panes
  or recreating an xterm restores the screen (`ensure_session` answers with the
  backlog and the session's spawn number, `Attached`). Output streams as
  `pty://output`; termination as `pty://exit`, with the spawn number.
- **ConPTY and the cursor.** portable-pty opens ConPTY with `INHERIT_CURSOR`:
  until the terminal answers `ESC[6n`, ConPTY lets out nothing. A helper with no
  pane would hang. The core cuts each `ESC[6n` out of the stream and answers
  with the cursor position per `vt100`; the query never reaches the scrollback.
- **The screen is drawn once and caught up.** The scrollback counts every byte
  ever written to it (`Scrollback::written`, the output's generation). Each
  session keeps one `vt100` terminal (`Drawn`) drawn to a generation at a size;
  a look at the screen (the activity sampler's Codex dialog check every 5 s,
  `has_draft`, `at_prompt`, `composer`, `luna_session_get`'s screen, a cursor
  query) feeds it only what was written since. It is drawn anew from the whole
  scrollback when the pty's size changed or what was written since has been
  trimmed off already. Locks: `Drawn`, then the scrollback, held only to copy
  out the new bytes; a look copies nothing under the session map. Both CLIs go
  through it. Kept, the terminal has seen output from before where the
  scrollback starts now, as a pane that stayed attached has.
- **What is read off disk is kept by stamp.** `sessions_meta` and the sampler
  ask after every live chat every few seconds. What they read of a file (a
  Claude Code registry entry, the titles in a transcript's tail; a rollout's
  tail, its first line's cwd and the limit its last turn met; Codex's
  `session_index.jsonl`) is kept by the file's length, change time and identity
  (inode, or creation time on Windows) (`stamped::ByStamp`) and read again once
  those change, and in any case once a minute, which catches a file a tool put
  back with its length and change time. A read that failed keeps nothing. A file
  changed in the last 2 s is read every time, as git treats a racily clean file:
  a change time is as coarse as the file system keeps it, and a registry entry
  going from `busy` to `idle` keeps its length. An entry unused for 10 minutes
  goes, swept at any look, and each cache has a cap. At idle a look is a `stat`
  per file and a read a minute.
- **Queues are bounded.** A session's input waits for its writer thread in a
  queue of at most 8 MB (`InputTx`): past that, keys, pastes and Luna's text
  are refused with an error, and so is everything after until the writer has
  caught up, so what goes in keeps its order with no gap; one write alone may
  be larger, and resizes always go. The first refusal of a stretch is logged
  and sent to the window as `pty://input-refused`, which the pane shows as
  `[keys not delivered: …]`: a keystroke is a send whose answer nobody reads.
  The core's own answers to its CLI's cursor queries (`InputTx::reply`) are
  never refused, or a CLI waiting on one would wait for good. Every byte bound
  takes its room in one step under a lock (`backlog.rs`). The reader hands
  output to the emitter through 512 reads (4 MB) at most; past that the
  reader waits, and the CLI's writes with it, which the log says once:
  output is never dropped or reordered.
- **Input to Codex as key presses.** ConPTY turns each character into a key
  press on the console's layout, and a character the layout lacks (on a Russian
  layout brackets, `< > ' ~ @ # $ ^ & |`, backtick, dashes) never reached Codex.
  For Codex the writer sends printable characters as win32-input-mode sequences
  (`ESC[0;0;<utf16>;1;0;1_` and the release; surrogate halves press only), and
  Ctrl+letter as the Latin letter with Ctrl held (`ESC[67;0;99;1;8;1_` for
  Ctrl+C). Backspace, Tab, Ctrl+J, Enter and escape sequences go as is. Claude
  Code reads text and needs none of this.
- Codex's thread is found by the terminal title: Luna starts it with
  `-c tui.terminal_title=["thread-id"]` and reads OSC 0/2 off the output
  ([providers](providers.md#reading-a-codex-session)).
- The composer and draft are read off the screen (`pty::read_composer`): the
  cursor stands at the caret in both CLIs; Claude Code's `❯` between two rules,
  Codex's `›` down to the first blank row, its placeholder left out.

## Invariants

- `claude/launch.rs` and `codex/launch.rs` are pure; everything with an effect
  happens in `pty/start.rs` (or `background/command.rs` / `background/run.rs`
  for a helper's turn) before them, under the chat's spawn gate.
- A session is registered (`register`) before its process is created. A chat
  the window starts is the person's, never a helper: it is registered with no
  parent and starts with no opening prompt, helper note, `--strict-mcp-config`
  or connectors turned off. Helpers start in `background/`.
- MCP tokens are never written to disk; the config names `${LUNA_MCP_TOKEN}`.
- `cwd` must exist.
- A delete (`PtyManager::delete`) holds the chat's spawn gate from before it
  takes the session until its cleanup is done: what it clears goes by the
  chat's id alone (the registry entry and token, the hooks and MCP files, the
  Codex rollout), and would clear a session spawned under that id meanwhile.
  A start of a deleted chat waits for the delete and is refused ("this chat
  was deleted"): the window never gives a chat id out twice, and a pane that
  had not let go would otherwise bring back a session the delete had just
  cleared. A worktree the chat ran in is left as it is: Luna removes none.
  Claude Code answers the quit in a worktree with "keep or remove?", and
  `shut_down` answers Enter, Keep.
- A session's exit goes by its chat id, with its spawn number. Whoever takes
  a session out of the map (a delete, a kill, a start over a session that
  exited by itself) waits under the gate, up to 5 s (`READER_WAIT`), for its
  reader thread to have said it exited (`ReaderEnd`). Each start gives the
  chat a new number before its launch (`next_generation`); the reader says
  the registry's exit and the `pty://exit` event under that number's lock
  and only while its number is the chat's, taking it away as it does, so
  one already saying it when the wait ran out is through before the start
  goes on, and one later says nothing. A taker done waiting (`retire`)
  says the exit itself, with the code if the process has one, where the
  number is still there: a reader stuck past the wait would otherwise leave
  the pane or the login window never told the session is gone. Each exit
  is said once: by the taker where the reader had not begun by the end of
  the wait, or began and comes to the word after the taker; by the reader
  where it came first. A
  respawn never hears the old session's exit, and the window applies an
  exit only to the session whose start answered with its number. The
  reader asks for the exit code by `try_wait` rather than holding the
  child's lock in a `wait`, which a shutdown asking whether it exited
  would block on.

## Failure modes

| Failure | Behavior |
| --- | --- |
| Hub not listening | hooks fail open; status falls back to the registry |
| ConPTY waiting on a cursor answer | answered by the core |
| Worktree of a resumed Codex chat gone | start fails, nothing re-creates it |
| Hook shell cannot parse the command | prevented by quoting; a failing hook lets the call go ahead |
