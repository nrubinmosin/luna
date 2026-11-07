# pty and launch

How a session is started, what it is started with, and how its terminal is run.

## Responsibility

- Build each CLI's command line and environment (pure functions, no I/O).
- Write what a start needs (hook settings, MCP config, instructions folder,
  token), then spawn in a pty.
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

Unit tests in `pty/start.rs` (sessions in a pane) and `background/command.rs`
(headless helpers) assert whole argument and environment lists, and the
generated settings files, against expected values written in the test.
`golden.rs` `Redact` replaces what changes per run (hub port and secrets, MCP
token, files under the data dir, a profile's random name) with placeholders
such as `<hook-url>`, `<settings-file>`, `<mcp-file>`. Any change to a launch
path shows up as a reviewed diff of the golden text.

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

- `claude/launch.rs` and `codex/launch.rs` write nothing; `pty/start.rs` does
  the writing under the chat's spawn gate.
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
