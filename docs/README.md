# Luna documentation

## User guide

- [The window and panes](user-guide/window.md): layouts, peek, groups, status
  bar, shortcuts, themes
- [Chats](user-guide/chats.md): the new-chat dialog for each CLI, status, titles,
  clipboard and links
- [Accounts](user-guide/accounts.md): account folders, live limits, trust,
  recommendations, recommended and shared settings
- [Skills and instructions](user-guide/library.md): the library, reaches,
  folders, copies that changed
- [Sessions and cleanup](user-guide/sessions.md): the core, quitting, restarts,
  orphans, worktrees, the log
- [Agents and MCP tools](user-guide/agents.md): helpers, talking to sessions,
  answers, notices, lifetime
- [Power and shutdown-when-done](user-guide/power.md)
- [Updates](user-guide/updates.md): Luna and the CLIs

## Architecture

- [Overview](architecture/overview.md): processes, IPC, data flow, module map
- [Agents](architecture/agents.md): MCP tools, registry, debts, notifier, rights
- [Background helpers](architecture/background-helpers.md): headless turns, and
  what outlives a core
- [pty and launch](architecture/pty-and-launch.md): command lines, hooks and the
  guard, golden tests, terminal quirks
- [Providers](architecture/providers.md): `claude/` and `codex/`, reading
  sessions, the Codex sandbox
- [Accounts and account files](architecture/accounts.md): limits, OAuth, trust,
  recommendations, safe writes
- [Library](architecture/library.md)
- [Worktrees](architecture/worktrees.md)
- [Power and lifecycle](architecture/power-and-lifecycle.md): busyness, jobs,
  keep-awake, core and window lifecycle, updates
- [Frontend](architecture/frontend.md): layout, stores, IPC modules, styles
- [CLI compatibility notes](cli-compatibility.md): observed Claude Code and Codex
  behavior by version

## Decisions (ADRs)

| ADR | Decision | Status |
| --- | --- | --- |
| [0001](adr/0001-session-state-lives-in-the-cli.md) | Session state lives in the CLI; the Rust core stays thin | Accepted |
| [0002](adr/0002-core-process-outlives-the-window.md) | Sessions run in a core process that outlives the window | Accepted |
| [0003](adr/0003-loopback-hub-for-hooks-and-mcp.md) | One loopback HTTP hub for hooks, the guard and MCP | Accepted |
| [0004](adr/0004-docker-only-toolchain.md) | Everything builds in Docker on pinned toolchains | Accepted |
| [0005](adr/0005-two-providers-as-independent-layers.md) | Claude Code and Codex are two independent layers | Accepted |
| [0006](adr/0006-isolated-test-flavor.md) | A test flavor isolated from the Luna in use | Accepted |
| [0007](adr/0007-least-privilege-for-agent-tools.md) | A least-privilege contract for agent tools | Accepted |
| [0008](adr/0008-strict-csp-with-inline-styles.md) | A strict CSP, with inline styles allowed | Accepted |
| [0009](adr/0009-modules-now-crates-later.md) | Modules now, crates later | Accepted |
| [0010](adr/0010-registry-encapsulation.md) | Encapsulate the agents Registry behind transition methods | Proposed |
| [0011](adr/0011-private-data-dir-beside-the-exe.md) | Luna's data lives in one private folder beside the exe | Accepted |
| [0012](adr/0012-agents-over-mcp-with-notices.md) | Agents drive sessions over MCP; answers come back as notices | Accepted |
| [0013](adr/0013-own-cli-binaries.md) | Luna ships and updates its own copies of the CLIs | Accepted |
| [0014](adr/0014-the-page-gets-only-what-it-calls.md) | The page gets only the commands it calls | Accepted |

New decisions start from the [template](adr/0000-template.md).

## Working on Luna

- [Development](development.md): toolchains, scripts, the gate, the test build,
  CI, adding a feature end to end
- [Testing](testing.md)
- [Releasing](releasing.md)
- [Contributing](../CONTRIBUTING.md) and [changelog](../CHANGELOG.md)
- [Security policy](../SECURITY.md) and [threat model](security/threat-model.md)
- [Brand assets](brand/README.md)
