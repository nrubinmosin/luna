# CLI compatibility notes

Luna reads undocumented files and screens of two CLIs
([ADR-0001](adr/0001-session-state-lives-in-the-cli.md)). This page collects
what was observed, by CLI and, where known, the version it was seen with. "All"
means seen across the versions Luna has shipped with and not tied to one.

When a CLI update breaks something, add the observation here with the version,
and check the other CLI for the same behavior (AGENTS.md, "Both providers").

## Claude Code

### Flags and launch (all)

| Observation | What Luna does |
| --- | --- |
| On exit, a clean worktree (no changes, no commits) is silently removed; given `--worktree <name>` again it is re-created with the same name and branch | a worktree chat resumes from the project folder as `--worktree <name> --resume <uuid>` |
| The trust prompt cannot be shown under `--worktree` | the new-chat dialog asks to trust the folder and writes `hasTrustDialogAccepted` into `.claude.json` before the spawn only when ticked; it will not create such a worktree chat unticked |
| `CLAUDE_CODE_SANDBOXED` set makes the process trust any folder, and also lifts other checks of project grants (2.1.318) | not used: no per-run trust for a Claude Code chat |
| `--mcp-config` takes any number of files and swallows a positional prompt as one more | the prompt goes after `--` |
| Without `"timeout"` on an HTTP MCP server, a call silent for 5 minutes is dropped | `"timeout": 1900000` |
| An MCP call longer than 120 s is moved to the background; the result arrives later as a task notification | the notifier waits 10 s for such a `wait` before sending its own notice |
| An MCP result past 25,000 tokens is cut | results held to 60,000 characters (`MAX_TEXT`) |
| A tool's own `timeout` is no deadline: past it the call moves to the background | the guard requires `timeout N` in the command itself |
| The stdout of a hook exiting 0 is added to the model's context | hooks print nothing |
| Only one `--settings` file is taken | hooks, guard, `skillOverrides` and `claudeMdExcludes` share one file |
| `--resume` does not continue a turn a killed process was in, unless an environment variable Luna does not set is present | the window restarts sessions between turns |
| `--append-system-prompt-file` is not re-read on resume (a resume keeps its system prompt) | folder instructions go as an `--add-dir` `CLAUDE.md` with `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD` |
| `claudeMdExcludes` takes User memory by absolute path | the account's `CLAUDE.md` path is excluded |
| `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD` is a switch: set, every additional folder's memory is read (2.1.318); `claudeMdExcludes` patterns are picomatch globs (`{ dot: true }`) on absolute paths with `/`, for User, Project and Local memory; every `\` in a pattern is turned into `/` first, so a backslash escapes nothing; a pattern also matches the path spelled exactly like it (2.1.319's bundled picomatch, `input === glob`); `! @ + ,` are literal unless an unescaped `(` or braces follow, `"` quotes | the variable only with the library's instructions; the other added folders' `CLAUDE.md`, `CLAUDE.local.md`, `.claude/CLAUDE.md` and `.claude/rules/**` excluded, each folder's name literal with `* ? [ ] { } ( ) |` and `"` alone in bracket classes (`[[]`); a pattern whose own spelling or meaning reaches this launch's instruction paths is dropped and shown as a Workaround. On a Claude Code upgrade, run the ignored test `claude_md_exclude_patterns_for_the_matcher_check` (`LUNA_GLOB_TABLE`) and check its table with the new binary's own picomatch |
| Skills under an `--add-dir` are listed by their own names; one whose name the repository or account already has is listed as `<folder>:<name>`, scoped to that folder | such copies are renamed `luna-<name>` |
| Before its first draw it syncs organization skills and plugins and makes the worktree, which can take minutes | `starting` state; sends refused until the prompt is taken |
| It copies `.claude/settings.local.json` into a new worktree; `.worktreeinclude` copies more | `as_made` allows that file only |
| It locks its worktree, and a killed one keeps the lock | removal is `git worktree remove -f -f` |
| Its cwd follows the shell's `cd` | worktree root found from anywhere inside |
| Each tool command runs in a session of its own (`detached`) | process jobs track more than a process group |
| `DISABLE_AUTOUPDATER=1` stops self-updates | set on every session |
| `copyOnSelect` is read only from `.claude.json` | such recommended keys are written there |
| OAuth refresh uses mkdir locks `<config>/.oauth_refresh.lock` and `<config>.lock` (stale after 60 s) and rotates refresh tokens | Luna refreshes under the same locks, writing only if the token on disk is unchanged |
| `--permission-prompt-tool` sends permission prompts and `AskUserQuestion` to an MCP tool | background helpers use `mcp__luna__luna_permission`, hidden by `--disallowedTools` |
| `--agent <name>` runs a session as a defined agent | `luna_spawn` `agent` |

### Files and status (all)

| Observation | What Luna does |
| --- | --- |
| Registry `<config>/sessions/<pid>.json`; `claude -p` writes one too | the cwd + start-time fallback takes only `entrypoint: cli` |
| The registry says busy while `isLoading \|\| delegatedActive`, so it stays busy while background subagents work | subagents tracked apart from the turn |
| A permission prompt answered "No" ends the turn with no `Stop`; Esc leaves a prompt with no `Stop` | a hook's "waiting" clears when the registry's idle is newer (`statusUpdatedAt`), else after 60 s |
| A turn cut off by an API error sends `StopFailure`, not `Stop`; the transcript's last line has `isApiErrorMessage` | session reads stalled, turn reported failed |
| The `Stop` hook can arrive before the last answer is in the transcript | a turn with no answer waits up to 5 s |
| A subagent's hooks carry its `agent_id`; `SubagentStart` / `SubagentStop` bracket it | turn and subagents kept apart |
| A question's `AskUserQuestion` call reaches the transcript only once answered | asks come from `PreToolUse` / `PermissionRequest` hooks |
| Titles: `ai-title` after the first message, `custom-title` on `/rename`, both rewritten into the tail; a second `ai-title` after a plan is accepted is the plan's handle (repeated by `agent-name`) | title order in [providers](architecture/providers.md#reading-a-claude-code-session) |
| Injected user lines (`<task-notification>`, interrupt marker, compaction summary) carry `turnOrigin` / `promptSource` where they say where they came from; `<synthetic>` replies exist | not counted as messages |
| A session resumed elsewhere keeps writing to the folder it started in | transcripts searched in every project folder |
| `claude -p` with a `run_in_background` task open waits for it and takes its end up in the same run | one notice for the whole job |
| `Unknown command: /x` is written and no turn starts | spawn answers `started: false` at once |
| Release bucket (seen on 2.1.319): `manifest.json` gives each platform's `checksum` (sha256) and `size`, and says `manifestSignatureEnforcement: flag`; `manifest.json.sig` beside it is an ASCII-armored OpenPGP v4 signature, RSA 4096 with SHA-512, by key `31DD DE24 DDFA B679 F42D 7BD2 BAA9 29FF 1A7E CACE` | the manifest verified against that key, pinned, before its checksum is used (`claude/manifest_sig.rs`) |
### Terminal (all)

| Observation | What Luna does |
| --- | --- |
| Composer: `❯` between two rules; cursor at the caret, hidden until the terminal reports focus | draft read off the screen; empty-prompt check does not use the cursor |
| Ctrl+S stashes a draft; it comes back with the next prompt ("Draft restored"); `› stashed` shows one is held | notices and sends stash a real draft, never over an existing stash |
| Image paste is Alt+V | Ctrl+V with an image presses Alt+V |
| `\` before Enter is a line break, not a submit | the draft counter knows it |

### Versions

| Version | Observation | What Luna does |
| --- | --- | --- |
| 2.1.277 | reads `AGENTS.md` when there is no `CLAUDE.md` | the repo has `AGENTS.md` only |
| 2.1.314 | a worktree starts at origin's default branch, not the folder's HEAD | Luna fetches origin first (optional); `as_made` reads the branch's creation commit from its reflog |
| 2.1.314 | `--worktree --dangerously-skip-permissions` reads the flag as the worktree name and turns permissions off | names always go as `--worktree=<name>` (`worktree::name_ok`, `worktree_arg`) |
| 2.1.318 | an agent's result is taken up in a turn of its own opening with `UserPromptSubmit` (`<task-notification>`) | the registry's busy is judged afresh after `SubagentStop` |
| 2.1.318+ | permission dialogs and lists drawn as Luna's screen recognition expects | an unrecognized dialog accepts only raw `keys` |
| 2.1.318 | a `Read(//<path>)` rule is rooted at `/`; on Windows a file's path is matched as `/c/Users/…` and a rule `//c/…` (or `C:/…`) is rooted at `C:\`; rules match without regard to case | the settings file denies `Read(//e/…/core/**)` |
| 2.1.319 | `--strict-mcp-config` keeps every MCP server but those of `--mcp-config` away, the account's claude.ai connectors included ("claude.ai connectors are not loaded in this Claude Code session (MCP servers are restricted to explicitly passed config here, e.g. --strict-mcp-config or --bare)"); the settings `disableClaudeAiConnectors: true` (honored from any scope) and `ENABLE_CLAUDEAI_MCP_SERVERS` turn the connectors off too | a helper starts with `--strict-mcp-config` and `disableClaudeAiConnectors` unless spawned with `connectors` |
| 2.1.319 | `autoMemoryEnabled: false` in a settings file turns auto-memory off ("Claude will not read from or write to the auto-memory directory"; the key the memory docs give); `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1` does too, and `=0` forces it on, ahead of the setting, from the environment or a settings file's `env` | a helper's `--settings` file says `false` and sets the variable to `1` in its `env`, and its environment does too |
| 2.1.318 | a transcript's assistant message that ended its turn carries `message.stop_reason: "end_turn"`; one that went on to a tool, or a line of a message still streaming, does not | that message is the turn's report (`Message::final_answer`); without one the report is picked by length and said to be (`reportGuessed`) |
| 2.1.319 | the version last checked live (MCP token from the environment, the signed manifest) | the oldest a fresh tree installs (`claude/cli.rs` `MIN_VERSION`); raise it with each version checked |

## Codex

### Flags and launch (all)

| Observation | What Luna does |
| --- | --- |
| Without `--no-daemon` a session may run in Codex's shared background server; `-c` overrides rule that out anyway, and Codex does not warn | `--no-daemon` always |
| `--worktree` checks the current commit out into `<CODEX_HOME>/worktrees/<4 hex>/<repo>` on a detached HEAD | path read from the rollout's `session_meta.cwd` |
| `--worktree` is refused on resume | resume as `resume <uuid> -C <worktree>` |
| `--approve-for-me` refuses `-a` / `-s` beside it | `auto-review` replaces both, in `workspace-write` |
| `never` + `danger-full-access` is spelled `--dangerously-bypass-approvals-and-sandbox` | passed as such |
| Project hooks need review ("Hooks need review") for every new worktree, account or edited hook; `--dangerously-bypass-hook-trust` skips it; `exec` drops untrusted hooks silently | "Hooks unasked" per folder; in a chat's terminal the list is an `ask`, the person's to answer |
| Hooks run through the user's shell (PowerShell on Windows, where a bare `@-` does not parse); a failing hook lets the call go ahead | the guard's command quotes `"@-"` and calls curl by full path |
| Hooks call its shell tool `Bash` | the guard matches `Bash` |
| MCP calls get 60 s by default | `tool_timeout_sec=1900`, `default_tools_approval_mode="approve"` |
| `-c developer_instructions=` replaces the value from the config stack (ancestors' `.codex` included) | Luna appends a note to helper prompts instead |
| Trust is matched by the checkout's exact path; a parent folder does not count; `-c projects=…` trusts for one run | trust per path, or a one-run override |
| The trust screen is followed by a Windows sandbox setup question | trust written before the spawn when the new-chat dialog's box is ticked; unticked, both are Codex's to ask |
| A `-c` key is split at each dot | plugin slugs are `[a-z0-9-]` |
| `skills.config` entries from `-c` are added to the account's selector by selector | folder hiding via one `-c skills.config=[…]` |
| `$CODEX_HOME/AGENTS.override.md` is read instead of `AGENTS.md` | reported in the library window |
| `check_for_update_on_startup = false` stops update checks | passed as `-c` on every start, chat and `exec` alike, never written to `config.toml` |
| `exec` refuses to run outside a Git repository without `--skip-git-repo-check` | passed to a helper only outside one |
| `approvals_reviewer` in the account's or project's config sends what Codex asks to a reviewer model | `-c approvals_reviewer="user"` for helpers only; a person's chat keeps its own |
| A refresh token is single-use; the server rejects a reused one ("refresh token was already used") | Luna never refreshes Codex tokens |
| Each tool command runs in a session of its own (`setsid`); `shell_environment_policy` may filter the environment, but `CODEX_THREAD_ID` is set after the filter | job membership also by `CODEX_THREAD_ID` |
| Its own continuation of an interrupted turn lives in the daemon; resumed, it says "Conversation interrupted" and writes no end for that turn | `scan_rollout` ignores a `task_started` older than the process |
| Has its own `spawn_agent` for subagents | Luna's instruction says those cannot use another provider or account |
| `codex-code-mode-host` and its `conhost` stay resident | not counted as background work while idle |

### Files and status (all)

| Observation | What Luna does |
| --- | --- |
| No live registry; one rollout per thread at `sessions/YYYY/MM/DD/rollout-<local time>-<uuid>.jsonl`, lines `{timestamp,type,payload}`; it appears only with the first turn | file matched via the terminal title, then by cwd and time |
| `session_meta.source` is `cli` for the TUI, `exec` for `codex exec`, `{"subagent":{"thread_spawn":{"parent_thread_id":…}}}` for subagents | only `cli` rollouts are guessed |
| `request_user_input` holds the turn; `request_user_input_async` stays open until the next `UserMessage` | both read as questions |
| Approval requests are not written to the rollout; the spinner redraws continuously | waiting = a turn in flight with a screen unchanged for 3 s, or a recognized approval dialog |
| Context: `last_token_usage.total_tokens − reasoning_output_tokens` against `model_context_window` | same formula |
| A token count is written again unchanged when rate limits renew | a Codex chat's model is the last `turn_context` after which the total grew |
| The rollout names a turn's model before asking it; a reroute leaves a `model rerouted` item | model taken only from a turn its model answered |
| `task_complete.last_agent_message`, `task_complete.error`, `turn_aborted` | last words and failures |
| `exec --json` reports retries as `error` events ("Reconnecting... 2/5") | not failures |
| `exec` reads stdin once | text to a background Codex helper waits for the next turn |
| Thread names in `session_index.jsonl`: the first line is Codex's; a differing one is `/rename` | titles |
| Model catalog in the account's `models_cache.json`; `codex debug models [--bundled]` | catalog fallbacks |
| Usage at `chatgpt.com/backend-api/wham/usage` with `auth.json`'s token; `token_count.rate_limits` in rollouts | limits and fallback |
| Package tarball (seen on rust-v0.167.0): regular files and folders only, no links; 52 entries and 581 MB unpacked for x86_64 Windows, 52 and 541 MB for x86_64 Linux, 50 and 427 MB for arm64 macOS. `codex-package_SHA256SUMS` covers the tarballs; Sigstore bundles (`*.tar.gz.sigstore`) are published for the Linux packages only. Written by Python's `tarfile` in PAX format (`scripts/codex_package/archive.py` in openai/codex), which adds a PAX record where a field does not fit a plain header | unpacked with entry, byte, metadata and stream caps; link entries and PAX `size` overrides refused (`codex/cli.rs`); checked against `SHA256SUMS` |

### Terminal (all)

| Observation | What Luna does |
| --- | --- |
| Under ConPTY each character becomes a key press on the console's layout; characters the layout lacks never arrive, and Ctrl+letter is resolved through the layout | printable characters and Ctrl+letter go as win32-input-mode sequences |
| Composer `›` down to the first blank row, with a dim placeholder | draft read off the screen |
| Image paste is Ctrl+V; no draft stash | sends wait for a draft to be sent or cleared |
| `-c tui.terminal_title=["thread-id"]` puts the thread id in the window title (OSC 0/2) | which rollout is ours |

### Windows sandbox (all)

| Observation | What Luna does |
| --- | --- |
| Without `windows.sandbox`, `read-only` and `workspace-write` reject or ask for every command, even `Get-Content` | `-c windows.sandbox="unelevated"`, unless the account's `config.toml` names one |
| Codex takes the first `pwsh.exe` on PATH; the Store app alias fails under the restricted token (`CreateProcessAsUserW failed: 5`) | PATH without `…\Microsoft\WindowsApps` |
| Under `workspace-write` Codex adds one ACL entry for its sandbox to the folder | documented |
| "No network" is only proxy variables at `127.0.0.1:9` and `GIT_SSH_COMMAND=cmd /c exit 1`; ssh, raw sockets and `gh` with those cleared still connect | documented: keeps tools off the network by accident, not data in |
| Under the restricted token schannel TLS fails (`SEC_E_NO_CREDENTIALS`) and Git's MSYS `sh.exe` / `ssh.exe` die (`CreateFileMapping`, error 5) | `http.sslBackend=openssl` appended to `GIT_CONFIG_*`; set `GIT_SSH` to Windows' `ssh.exe` |
| `network.enabled` in a permission profile is direct network, not `features.network_proxy` | `read-only-net` uses it |
| A config's rules for a profile name are laid over a `-c` profile of that name | `read-only-net`'s profile name is random per start |

### Versions

| Version | Observation | What Luna does |
| --- | --- | --- |
| before 0.157 | turn events were `turn_started` / `turn_complete` / `turn_aborted` | both spellings read |
| 0.157+ | `task_started` / `task_complete` | read |
| 0.166 | the terminal title item is cut to 29 characters, leaving 25 hex digits of the uuid | matched against the start of the file name's uuid; two alike are no answer |
| 0.166 | `/subagents` switches the title to the watched subagent | the session stays on its own rollout |
| 0.166 | subagents run inside the session's process, each with its own rollout naming its parent thread | `children_at_work` |
| 0.166 | a `read-only-net` profile beats `sandbox_mode` and sits beside `[sandbox_workspace_write]` without complaint: writes denied, network up | relied on |
| 0.167.0 | what Codex writes into a rollout as a user message no one typed is a block it recognizes by its wrapper (`CONTEXTUAL_USER_FRAGMENT_MATCHERS`, `core/src/context/contextual_user_message.rs`), tag names in any case: `<environment_context>`, `<skill>`, `<user_shell_command>`, `<turn_aborted>`, `<subagent_notification>`, `<recommended_plugins>`, `<agent_message_board_notification>`, `<codex_internal_context source=…>`, `<goal_context>`, `<hook_prompt …>`, `<external_KEY>`, `# AGENTS.md instructions for <dir>` … `</INSTRUCTIONS>` (older: `<user_instructions>`), and three `Warning: …` lines; a prompt of a person's may open with any other tag | `luna_read` and answers leave out exactly those blocks (`codex/session/read.rs` `injected`); a prompt such as `<task>…</task>` is kept and starts its turn |
| 0.167.0 | `-c mcp_servers.<name>.enabled=false` turns a server of the config off for the run (`codex mcp list` shows it `enabled: false`); there is no switch for all at once, and a `-c` naming a server no layer defines, or a name with a dot (a quoted key is not split as TOML would), fails the run: "failed to load bootstrap configuration" | a helper's run turns off each bare-key server of its account's `config.toml`; a project's and plugins' are left on |
| 0.167.0 | `features.apps` (stable, on by default with a ChatGPT login) is what offers ChatGPT's apps (connectors); `-c features.apps=false` turns it off for the run | set for every helper not spawned with `connectors` |
| 0.167.0 | an assistant message in a rollout carries `payload.phase`: `commentary`, a partial answer, or `final_answer` for the turn's terminal one (`MessagePhase`, protocol/src/models.rs) | the `final_answer` message is the turn's report; without one, picked by length and said to be |
| 0.167.0 | the account's `AGENTS.md` cannot be turned off for a session: `developer_instructions` only adds, `model_instructions_file` replaces the base prompt | folder instructions are Claude Code only |
| 0.167.0 | approval dialogs and lists drawn as Luna's screen recognition expects | an unrecognized dialog accepts only raw `keys` |
| 0.167.0 | `codex sandbox [-c …] [-P <profile> -C <dir>] -- <command>` runs one command in the sandbox those overrides give, with no login | the live test of `core/`'s reach uses it |
| 0.167.0 | a permission profile's `filesystem={"<dir>"="deny"}` (`"none"` the same) keeps reads under `<dir>` from a command under `:read-only` or `:workspace` on Linux; a dotted path inside the TOML value comes through whole | `read-only` and `read-only-net` deny `core/` off Windows |
| 0.167.0 | `extends=":workspace"` does not take the account's `[sandbox_workspace_write]`: its `writable_roots` and `network_access` are lost, as `sandbox_mode="workspace-write"` keeps them | `workspace-write` stays `--sandbox`, without the deny |
| 0.167.0 | on Windows, under `windows.sandbox="unelevated"`, any `"deny"` or `"none"` entry in a profile fails every command: "Restricted read-only access requires the elevated Windows sandbox backend" | no deny on Windows |
| 0.167.0 | on Windows the unelevated sandbox's token is restricted for writes only: a command reads what the user can, a folder with a protected DACL of the user, SYSTEM and Administrators included, and `read-only` still connects to loopback ports | documented (threat model F3) |
| 0.167.0 | the unelevated sandbox's token is restricted (`IsTokenRestricted`) for `read-only` and `workspace-write` both — the write access is an ACL grant on the workspace, not a looser token — and its integrity is Medium, the same as an unsandboxed process | the core's peer check keys on the restricted token, not integrity (`peer.rs`) |
| 0.167.0 | `codex sandbox`'s own argument parser makes `-C/--cd` require `-P/--permission-profile` ("required arguments were not provided: --permission-profile"; `cli/src/lib.rs` at `rust-v0.167.0`); its loader does accept a legacy `-c sandbox_mode="…"` override (`debug_sandbox.rs`). So the no-login runner takes `-c sandbox_mode="read-only"` with no `-C`, while a writable run uses `-P <profile>` (a config-file `[permissions.<p>] extends=":workspace"`) with `-C <dir>` | the peer-check live test drives workspace-write through a `:workspace` profile in a config file (argv nested quotes do not survive on Windows) |
