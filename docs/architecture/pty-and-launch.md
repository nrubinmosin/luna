# pty and launch

How a session is started, what it is started with, and how its terminal is run.

## Responsibility

- Lay out each CLI's command line and environment from what the start has
  settled: `claude/launch.rs` and `codex/launch.rs` are pure (they read no
  environment, disk or Luna state, and log, write, register or mint nothing).
- Settle a start under the chat's spawn gate (`pty/start.rs`; a helper's turn in
  `background/command.rs` through the same functions): the MCP token and
  registration, the settings, MCP and instructions files, the worktree and the
  library pass, what the sandbox and guard take from this machine; then spawn in
  a pty (`pty/spawn.rs`).
- Keep the scrollback, answer terminal queries, read the screen, and write
  input the way each CLI needs it.

## Key modules

| Path | What it holds |
| --- | --- |
| `pty/mod.rs` | the session map, `PtyManager`, `manager()` (the one per process) |
| `pty/start.rs` | `ensure_claude_session` / `ensure_codex_session`: what a start settles under the chat's spawn gate, then the command line |
| `pty/spawn.rs` | the shared spawn, scrollback, `pty://output` / `pty://exit`, cursor queries answered |
| `pty/control.rs` | write, resize, keys, kill, quitting the way a person would, delete, orphans |
| `pty/screen.rs` | the screen replayed (`vt100`), composer and draft, terminal modes and title, Codex key events on Windows; `SessionMeta` |
| `pty/probe.rs` | `ActivityProbe::working_dir()`, `session_meta` by provider, tails, sweep of hook and MCP files |
| `claude/launch.rs` | Claude Code's arguments and environment (`chat_args`), the settings file's hooks and guard, the helper's system prompt line |
| `codex/launch.rs` | Codex's arguments and environment, approval and sandbox overrides, `read-only-net`, the guard, Luna's MCP server |
| `guard.rs` | the PreToolUse guard against waits that never end |
| `golden.rs` | placeholders that make launch golden tests stable |

## Command lines

Flags are always set (so the CLI's own settings would be overridden): the
defaults readers fill the dialog from the files first, so the flags match them
unless changed by hand ([providers](providers.md#defaults)).

**Claude Code:**

```
claude --model <alias> --effort <low|medium|high|xhigh|max|ultracode>
       --permission-mode <mode> [--resume <uuid>] [--worktree[=<name>]]
       --settings <data>/hooks/<chat>.json [--mcp-config …] [--add-dir …] [-- <prompt>]
```

`cwd` is the chat's folder; `CLAUDE_CONFIG_DIR` is the account folder;
`DISABLE_AUTOUPDATER=1`. A worktree chat whose worktree is known resumes from
the project folder as `--worktree <name> --resume <uuid>` rather than starting
inside the worktree, because Claude Code removes a clean worktree on exit and,
given the name, makes it again with the same name and branch. `--resume` is
passed only when the transcript exists; otherwise the chat starts afresh.

**Codex:**

```
codex [resume <uuid>] -C <cwd> --no-daemon [--worktree] --model <m>
      -c model_reasoning_effort="<e>" -a <on-request|never> -s <sandbox> [-c …]
```

`CODEX_HOME` is the account folder. `never` + `danger-full-access` becomes
`--dangerously-bypass-approvals-and-sandbox`; `auto-review` is
`--approve-for-me` in place of `-a`/`-s`, in `workspace-write`.
`--no-daemon` keeps the session in this process and pty rather than in Codex's
shared server (which the `-c` overrides rule out anyway). Codex refuses
`--worktree` on resume, so a worktree chat resumes as `resume <uuid> -C
<worktree>`; a worktree gone fails the start. Login is the same command as
`codex login` in the account folder.

The core does not start a folder that does not exist: portable-pty would
silently substitute the user's profile.

## Golden tests

The goldens run the real start, not the builders alone.

- **In a pane** (`pty/start.rs`, `golden_claude_*`, `golden_codex_*`): the
  start runs through `ensure_*` and `pty::spawn` under the spawn gate, side
  effects and all, and `pty::spawn::capture` (test builds only) takes the
  `Launch` in place of creating the process. Each test compares the whole
  argument list, the environment, the cwd, the resume decision and the session's
  description, and reads back the settings, MCP and instructions files written
  for it. Cases: chats, permission modes, resume only with a transcript,
  worktrees by name, login, helpers with tools (guard, Luna's MCP, role), Codex
  approval / sandbox pairs, library folders (skills, instructions, plugins,
  hidden skills).
- **Headless** (`background/command.rs`, `golden_headless_*`): a helper turn's
  argv, folder and environment, and the settings and MCP files where the CLI takes them, for both CLIs: first turn,
  resume and worktree, sandboxes and the reviewer, tools, library folders. What
  goes on stdin (the prompt, Luna's note, slash commands kept out of a
  command's arguments) has tests of its own there.
- **Redaction is strict** (`golden.rs` `Redact`): only what changes per run is
  replaced, and each placeholder is first checked to be what it stands for. The
  hub's URLs, the data-dir files and the notes folder become `<hook-url>`,
  `<guard-url>`, `<mcp-url>`, `<settings-file>`, `<mcp-file>`,
  `<instructions-dir>`, `<notes-dir>`; `LUNA_MCP_TOKEN` becomes `<token>` only
  once it is the token the registry knows this session by; a sandbox's `PATH`
  becomes `<sandbox-path>` only if it is this process's minus the Store's app
  aliases; the `GIT_CONFIG_*` TLS entries (Windows) become one
  `<git-ssl-backend>` only if they are exactly this process's plus the backend;
  a `read-only-net` profile's random name becomes `<profile>`.

Any change to a launch path shows up as a reviewed diff of the expected values.

## Hooks and the guard

Every Claude Code session starts with a `--settings` file adding hooks that post
to the hub with `curl.exe` (elsewhere `curl`). Hooks print nothing: Claude Code
adds the stdout of a hook exiting 0 to the context. The one settings file also
carries the library's `skillOverrides` and `claudeMdExcludes`
(`claude_settings_file`), since only one `--settings` flag is taken.

The **guard** (`guard.rs`) is a PreToolUse hook on `Bash|PowerShell|Monitor` for
a chat with tools and every helper; for Codex `-c hooks.PreToolUse=[…]` on its
shell (Codex calls it `Bash`), only where hooks run unasked. It posts to
`/guard/<id>/<secret>`, which denies a command that follows a file
(`tail -f/-F/--follow`, `Get-Content -Wait`; quotes, comments, heredocs and
`bash -c` parsed as a shell would) with no `timeout N` in front of it, with
`permissionDecision: "deny"` and what to do instead; anything else gets an empty
204. A tool's own `timeout` is no deadline (Claude Code moves the call to the
background). With the hub down the call goes ahead. Codex runs a hook through
the user's shell (PowerShell on Windows, where a bare `@-` does not parse), so
the command quotes `"@-"` and calls curl by full path.

## Scrollback and terminal state

- The output buffer (2 MB cap) lives in the core, so moving a chat between panes
  or recreating an xterm restores the screen (`ensure_session` returns the
  backlog). Output streams as `pty://output`; termination as `pty://exit`.
- **ConPTY and the cursor.** portable-pty opens ConPTY with `INHERIT_CURSOR`:
  until the terminal answers `ESC[6n`, ConPTY lets out nothing. A helper with no
  pane would hang. The core cuts each `ESC[6n` out of the stream and answers
  with the cursor position per `vt100`; the query never reaches the scrollback.
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
- A session is registered (`register`, and `spawn_is` for an agent's spawn)
  before its process is created.
- MCP tokens are never written to disk; the config names `${LUNA_MCP_TOKEN}`.
- A prompt goes after `--`: Claude Code's `--mcp-config` takes any number of
  files and swallowed the prompt as one more.
- `cwd` must exist.

## Failure modes

| Failure | Behavior |
| --- | --- |
| Hub not listening | hooks fail open; status falls back to the registry |
| ConPTY waiting on a cursor answer | answered by the core |
| Worktree of a resumed Codex chat gone | start fails, nothing re-creates it |
| Hook shell cannot parse the command | prevented by quoting; a failing hook lets the call go ahead |
